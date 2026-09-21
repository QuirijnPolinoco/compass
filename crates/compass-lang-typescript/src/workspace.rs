//! Workspace (monorepo) package resolution: `import { x } from "@acme/shared"` → the source
//! file of the in-repo package named `@acme/shared`.
//!
//! Without this, every import that crosses a package boundary looks like a third-party
//! dependency, so a monorepo's most important edges — app → shared library — are missing and
//! the blast radius of a shared file stops at its own package.
//!
//! `package.json` is read as **resolver input**, never mapped as a node (ADR-0007 §4). Packages
//! are discovered from the directories that hold mapped source files, so no `workspaces` glob
//! has to be interpreted and `node_modules` (ignored, hence unmapped) can never match.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use compass_extract::ResolutionContext;

use crate::{normalize, parent_dir, resolve_path};

/// Build-output directories a package's entry fields usually point into, tried as `src/` —
/// the map holds sources, and `dist/` is both ignored and absent before a build.
const OUTPUT_DIRS: [&str; 5] = ["dist", "lib", "build", "out", "esm"];

/// `exports` conditions, most source-like first.
const CONDITIONS: [&str; 7] = [
    "source", "types", "import", "module", "default", "require", "node",
];

/// Entry fields of a `package.json`, most source-like first.
const ENTRY_FIELDS: [&str; 5] = ["source", "types", "typings", "module", "main"];

/// One in-repo package.
struct Package {
    /// Repo-relative directory holding its `package.json` (empty = repo root).
    dir: String,
    manifest: serde_json::Value,
}

/// The in-repo packages by name, and whether the repo declares itself a workspace.
pub(crate) struct Workspace {
    packages: HashMap<String, Package>,
    /// A root `workspaces` field, `pnpm-workspace.yaml` or `lerna.json`. With one, a package-name
    /// import is how the package manager itself links the packages (a certain edge). Without
    /// one, an in-repo package that shares a name with an import is only a convention.
    pub(crate) declared: bool,
}

impl Workspace {
    /// The workspace of `ctx`'s repo, built once per repo root.
    pub(crate) fn of(ctx: &dyn ResolutionContext) -> Arc<Workspace> {
        static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Workspace>>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let mut guard = cache.lock().expect("workspace cache poisoned");
        if let Some(hit) = guard.get(ctx.repo_root()) {
            return hit.clone();
        }
        let built = Arc::new(Workspace::discover(ctx));
        guard.insert(ctx.repo_root().to_path_buf(), built.clone());
        built
    }

    fn discover(ctx: &dyn ResolutionContext) -> Workspace {
        let root = ctx.repo_root();

        // Every directory that holds (or is an ancestor of) a mapped file may be a package root.
        let mut dirs: BTreeSet<String> = BTreeSet::from([String::new()]);
        for file in ctx.all_files() {
            let mut dir = parent_dir(&normalize(file));
            while !dir.is_empty() && dirs.insert(dir.clone()) {
                dir = parent_dir(&dir);
            }
        }

        let mut packages = HashMap::new();
        for dir in dirs {
            let Some(manifest) = read_json(&root.join(&dir).join("package.json")) else {
                continue;
            };
            if let Some(name) = manifest.get("name").and_then(|n| n.as_str()) {
                // BTreeSet order: on a duplicate name the shallowest/first directory wins.
                packages
                    .entry(name.to_string())
                    .or_insert(Package { dir, manifest });
            }
        }

        let declared = read_json(&root.join("package.json"))
            .is_some_and(|m| m.get("workspaces").is_some())
            || root.join("pnpm-workspace.yaml").is_file()
            || root.join("lerna.json").is_file();

        Workspace { packages, declared }
    }

    /// Candidate repo-relative base paths for a bare specifier that names an in-repo package
    /// (`@acme/shared`, `@acme/shared/utils`), best first. Empty if it names none.
    pub(crate) fn resolve(&self, spec: &str) -> Vec<String> {
        let (name, subpath) = split_specifier(spec);
        let Some(package) = self.packages.get(name) else {
            return Vec::new();
        };

        let mut targets: Vec<String> = Vec::new();
        let export_key = if subpath.is_empty() {
            ".".to_string()
        } else {
            format!("./{subpath}")
        };
        targets.extend(export_targets(&package.manifest, &export_key));
        if subpath.is_empty() {
            for field in ENTRY_FIELDS {
                if let Some(entry) = package.manifest.get(field).and_then(|v| v.as_str()) {
                    targets.push(entry.to_string());
                }
            }
            targets.push("src/index".to_string());
            targets.push("index".to_string());
        } else {
            targets.push(subpath.to_string());
            targets.push(format!("src/{subpath}"));
        }

        let mut bases = Vec::new();
        for target in &targets {
            let target = strip_js_extension(target.trim_start_matches("./"));
            // The source counterpart of a build output first: `dist/index` → `src/index`.
            if let Some((first, rest)) = target.split_once('/') {
                if OUTPUT_DIRS.contains(&first) {
                    bases.push(resolve_path(&package.dir, &format!("src/{rest}")));
                }
            }
            bases.push(resolve_path(&package.dir, target));
        }
        bases.dedup();
        bases
    }
}

/// `@scope/name/sub/path` → (`@scope/name`, `sub/path`); `name/sub` → (`name`, `sub`).
fn split_specifier(spec: &str) -> (&str, &str) {
    let name_segments = if spec.starts_with('@') { 2 } else { 1 };
    match spec.match_indices('/').nth(name_segments - 1) {
        Some((i, _)) => (&spec[..i], &spec[i + 1..]),
        None => (spec, ""),
    }
}

/// The file(s) `exports[key]` points at: a string, or a (possibly nested) conditions object.
/// A `./*` pattern key is matched when there is no exact one.
fn export_targets(manifest: &serde_json::Value, key: &str) -> Vec<String> {
    let Some(exports) = manifest.get("exports") else {
        return Vec::new();
    };
    // `"exports": "./src/index.ts"` and `"exports": { "import": … }` both describe ".".
    let is_subpath_map = exports
        .as_object()
        .is_some_and(|o| o.keys().any(|k| k.starts_with('.')));
    if !is_subpath_map {
        return if key == "." {
            condition_targets(exports)
        } else {
            Vec::new()
        };
    }

    if let Some(exact) = exports.get(key) {
        return condition_targets(exact);
    }
    let Some(map) = exports.as_object() else {
        return Vec::new();
    };
    for (pattern, value) in map {
        let Some((prefix, suffix)) = pattern.split_once('*') else {
            continue;
        };
        if key.len() >= prefix.len() + suffix.len()
            && key.starts_with(prefix)
            && key.ends_with(suffix)
        {
            let captured = &key[prefix.len()..key.len() - suffix.len()];
            return condition_targets(value)
                .into_iter()
                .map(|t| t.replace('*', captured))
                .collect();
        }
    }
    Vec::new()
}

fn condition_targets(value: &serde_json::Value) -> Vec<String> {
    match value {
        serde_json::Value::String(path) => vec![path.clone()],
        serde_json::Value::Object(conditions) => CONDITIONS
            .iter()
            .filter_map(|c| conditions.get(*c))
            .flat_map(condition_targets)
            .collect(),
        _ => Vec::new(),
    }
}

/// Drop a JS/TS/declaration extension so the caller can try every source extension.
fn strip_js_extension(path: &str) -> &str {
    for ext in [
        ".d.ts", ".d.mts", ".d.cts", ".mjs", ".cjs", ".mts", ".cts", ".jsx", ".tsx", ".js", ".ts",
    ] {
        if let Some(stem) = path.strip_suffix(ext) {
            return stem;
        }
    }
    path
}

fn read_json(path: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn split_specifier_separates_the_package_name_from_the_subpath() {
        assert_eq!(split_specifier("@acme/shared"), ("@acme/shared", ""));
        assert_eq!(
            split_specifier("@acme/shared/utils/date"),
            ("@acme/shared", "utils/date")
        );
        assert_eq!(split_specifier("shared"), ("shared", ""));
        assert_eq!(split_specifier("shared/utils"), ("shared", "utils"));
    }

    #[test]
    fn export_targets_reads_strings_conditions_and_patterns() {
        let manifest = json!({ "exports": {
            ".": { "import": "./dist/index.mjs", "types": "./dist/index.d.ts" },
            "./config": "./src/config.ts",
            "./features/*": { "default": "./dist/features/*.js" }
        }});
        // Conditions come back most source-like first.
        assert_eq!(
            export_targets(&manifest, "."),
            ["./dist/index.d.ts", "./dist/index.mjs"]
        );
        assert_eq!(export_targets(&manifest, "./config"), ["./src/config.ts"]);
        assert_eq!(
            export_targets(&manifest, "./features/login"),
            ["./dist/features/login.js"]
        );
        assert!(export_targets(&manifest, "./missing").is_empty());

        // The sugar forms describe "." only.
        let sugar = json!({ "exports": "./src/main.ts" });
        assert_eq!(export_targets(&sugar, "."), ["./src/main.ts"]);
        assert!(export_targets(&sugar, "./x").is_empty());
    }

    #[test]
    fn strip_js_extension_handles_declaration_files() {
        assert_eq!(strip_js_extension("dist/index.d.ts"), "dist/index");
        assert_eq!(strip_js_extension("dist/index.mjs"), "dist/index");
        assert_eq!(strip_js_extension("src/index"), "src/index");
    }
}
