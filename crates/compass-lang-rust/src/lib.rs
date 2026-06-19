//! `compass-lang-rust` — the Rust language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.rs), the tree-sitter-rust grammar, symbol extraction (functions, methods,
//! structs, enums, unions, traits, type aliases, consts, modules), and import resolution.
//!
//! Two kinds of Rust dependency become edges:
//! - **Intra-crate**: `mod foo;` resolves to a file (`foo.rs` or `foo/mod.rs`) by the 2018
//!   module convention.
//! - **Cross-crate**: `use <crate>::…` / `extern crate <crate>;` **and** fully-qualified path
//!   references with no `use` (e.g. `apis_core::Price(..)`) resolve to that crate's library
//!   root (`…/src/lib.rs`) when `<crate>` is a member of the Cargo workspace. The
//!   crate-name → lib-root map is read once from the `Cargo.toml`s under the repo root.
//!   `std`/third-party crates resolve as [`ResolvedImport::External`] (no edge, no
//!   diagnostic); `crate`/`self`/`super` self-references and non-crate path roots
//!   (`String::new`, `Vec::<T>`) are ignored.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The Rust extractor. Registered by the CLI composition root (ADR-0003).
pub struct RustExtractor;

impl Extractor for RustExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("rust")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["rs"],
            shebangs: &[],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_rust::LANGUAGE.into()
    }

    fn extract(&self, source: &[u8], tree: &Tree) -> Extraction {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        visit(tree.root_node(), source, false, &mut symbols, &mut imports);
        // A file may `use` the same crate many times; one edge per crate is enough.
        dedup_crate_imports(&mut imports);
        Extraction { symbols, imports }
    }

    fn resolve(
        &self,
        imports: &[RawImport],
        ctx: &dyn ResolutionContext,
        _config: &LangConfig,
    ) -> Vec<ResolvedImport> {
        let current = normalize(ctx.current_file());
        let dir = parent_dir(&current);
        let base = module_base(dir, file_stem(&current));
        let crate_index = crate_index_for(ctx.repo_root());

        imports
            .iter()
            .map(|imp| {
                // Cross-crate `use <crate>::…` (marked with the `use:` sentinel in extract).
                if let Some(crate_name) = imp.specifier.strip_prefix(USE_PREFIX) {
                    return resolve_crate_use(crate_name, imp.span, &crate_index, ctx);
                }
                // Intra-crate `mod foo;` → a file in this crate's module tree.
                let name = imp.specifier.as_str();
                let candidates = [
                    join(&base, &format!("{name}.rs")),
                    join(&base, &format!("{name}/mod.rs")),
                ];
                for cand in &candidates {
                    if let Some(target) = ctx.file_by_path(Path::new(cand)) {
                        return ResolvedImport::Resolved {
                            target,
                            span: imp.span,
                        };
                    }
                }
                ResolvedImport::Unresolved {
                    specifier: imp.specifier.clone(),
                    span: imp.span,
                    reason: "`mod` declaration resolves to no .rs file".to_string(),
                }
            })
            .collect()
    }
}

/// Sentinel prefix marking a `RawImport` as a cross-crate `use`/`extern crate` root rather
/// than a `mod` declaration. `:` can't appear in a Rust identifier, so it can't collide with
/// a module name.
const USE_PREFIX: &str = "use:";

/// A repo's crate-name → repo-relative lib-root map.
type CrateIndex = HashMap<String, String>;
/// Per-repo-root cache of [`CrateIndex`] (keyed by repo root, so one process can map several).
type IndexCache = HashMap<PathBuf, Arc<CrateIndex>>;

/// Resolve `use <crate>::…`: an edge to the crate's lib root when it's a workspace member,
/// otherwise `External` (std / third-party) — never a broken-import diagnostic.
fn resolve_crate_use(
    crate_name: &str,
    span: Span,
    crate_index: &CrateIndex,
    ctx: &dyn ResolutionContext,
) -> ResolvedImport {
    if let Some(lib_rel) = crate_index.get(crate_name) {
        if let Some(target) = ctx.file_by_path(Path::new(lib_rel)) {
            return ResolvedImport::Resolved { target, span };
        }
    }
    ResolvedImport::External {
        specifier: crate_name.to_string(),
    }
}

/// Drop duplicate cross-crate imports (keep the first per crate); `mod` imports pass through.
fn dedup_crate_imports(imports: &mut Vec<RawImport>) {
    let mut seen = HashSet::new();
    imports.retain(|imp| match imp.specifier.strip_prefix(USE_PREFIX) {
        Some(crate_name) => seen.insert(crate_name.to_string()),
        None => true,
    });
}

/// The crate-name → repo-relative lib-root map for a repo, built once and cached per repo
/// root (the resolve phase runs per file, but the Cargo layout is constant for a run).
fn crate_index_for(repo_root: &Path) -> Arc<CrateIndex> {
    static CACHE: OnceLock<Mutex<IndexCache>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("crate-index cache poisoned");
    guard
        .entry(repo_root.to_path_buf())
        .or_insert_with(|| Arc::new(build_crate_index(repo_root)))
        .clone()
}

/// Build `import_name → repo-relative lib root` from the Cargo workspace under `repo_root`.
/// Reads the root `Cargo.toml` for `[workspace] members` (and a root `[package]`, for a
/// single-crate repo), then each member's `[package] name` + lib path. Best-effort: any
/// unreadable/odd manifest is skipped, never fatal.
fn build_crate_index(repo_root: &Path) -> CrateIndex {
    let mut index = CrateIndex::new();
    let Ok(text) = std::fs::read_to_string(repo_root.join("Cargo.toml")) else {
        return index;
    };
    let Ok(root) = text.parse::<toml::Value>() else {
        return index;
    };

    let mut member_dirs: Vec<String> = Vec::new();
    if let Some(members) = root
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
    {
        for member in members.iter().filter_map(|m| m.as_str()) {
            expand_member(repo_root, member, &mut member_dirs);
        }
    }
    // A plain (non-workspace) crate: the root itself is a package.
    if root.get("package").is_some() {
        member_dirs.push(String::new());
    }

    for dir in member_dirs {
        if let Some((import_name, lib_rel)) = crate_lib_root(repo_root, &dir) {
            index.entry(import_name).or_insert(lib_rel);
        }
    }
    index
}

/// Expand one `members` entry into concrete repo-relative directories, supporting a trailing
/// `/*` glob (the common `crates/*` form).
fn expand_member(repo_root: &Path, member: &str, out: &mut Vec<String>) {
    let member = member.trim_end_matches('/');
    if let Some(prefix) = member.strip_suffix("/*") {
        if let Ok(entries) = std::fs::read_dir(repo_root.join(prefix)) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        out.push(format!("{prefix}/{name}"));
                    }
                }
            }
        }
    } else {
        out.push(member.to_string());
    }
}

/// For a crate directory (empty = repo root), read its `Cargo.toml` and return
/// `(import_name, repo-relative lib root)` — but only if that lib root actually exists (so a
/// bin-only crate, which can't be `use`d, is excluded).
fn crate_lib_root(repo_root: &Path, dir: &str) -> Option<(String, String)> {
    let manifest = if dir.is_empty() {
        repo_root.join("Cargo.toml")
    } else {
        repo_root.join(dir).join("Cargo.toml")
    };
    let value = std::fs::read_to_string(&manifest)
        .ok()?
        .parse::<toml::Value>()
        .ok()?;
    let name = value.get("package")?.get("name")?.as_str()?;
    // The crate's import name is its package name with `-` → `_`.
    let import_name = name.replace('-', "_");
    let lib_path = value
        .get("lib")
        .and_then(|l| l.get("path"))
        .and_then(|p| p.as_str())
        .unwrap_or("src/lib.rs");
    let lib_rel = if dir.is_empty() {
        lib_path.to_string()
    } else {
        format!("{dir}/{lib_path}")
    }
    .replace('\\', "/");

    repo_root
        .join(&lib_rel)
        .is_file()
        .then_some((import_name, lib_rel))
}

/// Where submodules of a file live (Rust 2018): a `mod`/`lib`/`main` file's submodules sit
/// in its own directory; any other file `x.rs` nests its submodules under `x/`.
fn module_base(dir: &str, stem: &str) -> String {
    if matches!(stem, "lib" | "main" | "mod") {
        dir.to_string()
    } else {
        join(dir, stem)
    }
}

fn join(root: &str, path: &str) -> String {
    if root.is_empty() {
        path.to_string()
    } else {
        format!("{root}/{path}")
    }
}

fn parent_dir(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

fn file_stem(rel: &str) -> &str {
    let name = rel.rsplit_once('/').map(|(_, n)| n).unwrap_or(rel);
    name.strip_suffix(".rs").unwrap_or(name)
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// The crate root of a `use` declaration (the first path segment), or `None` for a
/// self-reference (`crate`/`self`/`super`) or a glob/list with no leading crate.
fn use_crate_root(node: Node, src: &[u8]) -> Option<String> {
    let argument = node.child_by_field_name("argument")?;
    crate_root_of(argument.utf8_text(src).ok()?)
}

/// Extract the leading crate identifier from a `use` path's text, e.g.
/// `compass_core::{Graph, MapQuery}` → `compass_core`, `foo as bar` → `foo`.
fn crate_root_of(path_text: &str) -> Option<String> {
    let seg = path_text
        .trim()
        .trim_start_matches("::")
        .split("::")
        .next()?
        .split_whitespace()
        .next()?
        .trim();
    if seg.is_empty()
        || !seg
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
    {
        return None; // `{ … }` group, `*` glob, etc.
    }
    match seg {
        "crate" | "self" | "super" => None,
        _ => Some(seg.to_string()),
    }
}

/// The crate named by an `extern crate <name>;` declaration.
fn extern_crate_root(node: Node, src: &[u8]) -> Option<String> {
    let name = node.child_by_field_name("name")?;
    Some(name.utf8_text(src).ok()?.to_string())
}

fn visit(
    node: Node,
    src: &[u8],
    in_impl: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    imports: &mut Vec<RawImport>,
) {
    match node.kind() {
        "function_item" => {
            let kind = if in_impl {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            push_named(node, "name", kind, src, symbols);
            recurse(node, src, false, symbols, imports);
            return;
        }
        "impl_item" => {
            recurse(node, src, true, symbols, imports);
            return;
        }
        "trait_item" => {
            push_named(node, "name", SymbolKind::Interface, src, symbols);
            recurse(node, src, true, symbols, imports);
            return;
        }
        "mod_item" => {
            push_named(node, "name", SymbolKind::Module, src, symbols);
            // `mod foo;` (no inline body) pulls in a file; `mod foo { .. }` does not.
            if !has_child_kind(node, "declaration_list") {
                if let Some(name) = node.child_by_field_name("name") {
                    if let Ok(text) = name.utf8_text(src) {
                        imports.push(RawImport {
                            specifier: text.to_string(),
                            span: span_of(name),
                        });
                    }
                }
            }
            recurse(node, src, false, symbols, imports);
            return;
        }
        "use_declaration" => {
            // Cross-crate dependency: the first path segment is the crate root.
            if let Some(root) = use_crate_root(node, src) {
                imports.push(RawImport {
                    specifier: format!("{USE_PREFIX}{root}"),
                    span: span_of(node),
                });
            }
            return; // a `use` declares no symbols
        }
        "extern_crate_declaration" => {
            if let Some(root) = extern_crate_root(node, src) {
                imports.push(RawImport {
                    specifier: format!("{USE_PREFIX}{root}"),
                    span: span_of(node),
                });
            }
            return;
        }
        "struct_item" | "union_item" => push_named(node, "name", SymbolKind::Struct, src, symbols),
        "enum_item" => push_named(node, "name", SymbolKind::Enum, src, symbols),
        "type_item" => push_named(node, "name", SymbolKind::Other, src, symbols),
        "const_item" | "static_item" => {
            push_named(node, "name", SymbolKind::Constant, src, symbols)
        }
        // Cross-crate refs written as a fully-qualified PATH with no `use`, e.g.
        // `apis_core::Price(..)`. Capture the leading segment when it looks like a crate
        // (lowercase — so `String::new`/`Vec::<T>`/`Self::` are skipped); `resolve` keeps
        // only the ones that are workspace crates. Falls through to recurse, so a crate ref
        // nested in generics (`Vec<other::Thing>`) is still caught.
        "scoped_identifier" | "scoped_type_identifier" => {
            if let Some(root) = node
                .utf8_text(src)
                .ok()
                .and_then(crate_root_of)
                .filter(|r| r.starts_with(|c: char| c.is_ascii_lowercase()))
            {
                imports.push(RawImport {
                    specifier: format!("{USE_PREFIX}{root}"),
                    span: span_of(node),
                });
            }
        }
        _ => {}
    }
    recurse(node, src, in_impl, symbols, imports);
}

fn recurse(
    node: Node,
    src: &[u8],
    in_impl: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    imports: &mut Vec<RawImport>,
) {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            visit(child, src, in_impl, symbols, imports);
        }
        i += 1;
    }
}

fn has_child_kind(node: Node, kind: &str) -> bool {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            if child.kind() == kind {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn push_named(
    node: Node,
    field: &str,
    kind: SymbolKind,
    src: &[u8],
    symbols: &mut Vec<ExtractedSymbol>,
) {
    if let Some(name_node) = node.child_by_field_name(field) {
        if let Ok(name) = name_node.utf8_text(src) {
            symbols.push(ExtractedSymbol {
                name: name.to_string(),
                kind,
                span: span_of(name_node),
            });
        }
    }
}

fn span_of(node: Node) -> Span {
    let start = node.start_position();
    Span {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_row: start.row,
        start_col: start.column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use compass_core::SymbolKind::{
        Constant, Enum, Function, Interface, Method, Module, Other, Struct,
    };
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    /// A sample exercising every symbol kind the extractor recognises plus both file-backed
    /// (`mod foo;`) and inline (`mod foo { .. }`) module declarations.
    const SAMPLE: &str = r#"
mod util;
mod missing;

pub mod inline {
    pub fn helper() {}
}

use std::collections::HashMap;

pub struct Point {
    x: i32,
}

pub union Bits {
    int: u32,
    float: f32,
}

pub enum Color {
    Red,
    Green,
}

pub type Pair = (i32, i32);

pub const MAX: u32 = 10;

static GREETING: &str = "hi";

pub trait Greeter {
    fn greet(&self) -> String;
}

impl Greeter for Point {
    fn greet(&self) -> String {
        String::new()
    }
}

impl Point {
    fn x(&self) -> i32 {
        self.x
    }
}

fn main() {
    let _ = util::run();
}
"#;

    fn extract(src: &str) -> Extraction {
        let x = RustExtractor;
        let tree = compass_extract::parse(&x.grammar(), src.as_bytes()).expect("parse");
        x.extract(src.as_bytes(), &tree)
    }

    fn raw(specifier: &str) -> RawImport {
        RawImport {
            specifier: specifier.to_string(),
            span: Span {
                start_byte: 0,
                end_byte: 0,
                start_row: 0,
                start_col: 0,
            },
        }
    }

    #[test]
    fn extracts_exactly_the_expected_symbols() {
        let mut got: Vec<(String, SymbolKind)> = extract(SAMPLE)
            .symbols
            .into_iter()
            .map(|s| (s.name, s.kind))
            .collect();
        got.sort();
        let mut want = vec![
            // `mod foo;` / `mod foo { .. }` declarations are Module symbols.
            ("util".to_string(), Module),
            ("missing".to_string(), Module),
            ("inline".to_string(), Module),
            // A `fn` inside an inline module is a free Function (not a Method).
            ("helper".to_string(), Function),
            // struct / union both map to Struct; enum to Enum.
            ("Point".to_string(), Struct),
            ("Bits".to_string(), Struct),
            ("Color".to_string(), Enum),
            // type alias -> Other.
            ("Pair".to_string(), Other),
            // const and static both map to Constant.
            ("MAX".to_string(), Constant),
            ("GREETING".to_string(), Constant),
            // trait -> Interface; its required fn is a Method (trait body counts as impl).
            ("Greeter".to_string(), Interface),
            ("greet".to_string(), Method),
            // an inherent-impl fn is also a Method.
            ("x".to_string(), Method),
            // a free top-level fn is a Function.
            ("main".to_string(), Function),
        ];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn extracts_mod_imports_in_order() {
        // File-backed `mod foo;` declarations only (bare specifiers, no `use:` sentinel), in
        // source order; the inline `mod inline { .. }` is not an import.
        let mods: Vec<String> = extract(SAMPLE)
            .imports
            .into_iter()
            .map(|i| i.specifier)
            .filter(|s| !s.starts_with(USE_PREFIX))
            .collect();
        assert_eq!(mods, ["util", "missing"]);
    }

    #[test]
    fn captures_path_qualified_crate_usage_without_use() {
        // A crate referenced via a fully-qualified path with NO `use` — the case Compass used
        // to miss. `String::new()` (uppercase) is not captured; `self.x` is not a crate path.
        let src = r#"
fn build() -> apis_core::Money {
    let p = apis_core::Price(1);
    let v: Vec<other_crate::Tick> = Vec::new();
    String::new();
    p
}
"#;
        let specs: Vec<String> = extract(src)
            .imports
            .into_iter()
            .map(|i| i.specifier)
            .collect();
        assert!(specs.contains(&"use:apis_core".to_string()));
        assert!(specs.contains(&"use:other_crate".to_string())); // nested in a generic
        assert!(!specs.iter().any(|s| s.contains("String")));
        // Deduped: `apis_core` appears 3× in source but once in imports.
        assert_eq!(specs.iter().filter(|s| *s == "use:apis_core").count(), 1);
    }

    #[test]
    fn captures_crate_uses_and_skips_self_references() {
        let src = r#"
use compass_core::Graph;
use compass_core::MapQuery;
use serde::Serialize;
use crate::registry;
use self::foo::Bar;
use super::baz;
extern crate libc;
"#;
        let specs: Vec<String> = extract(src)
            .imports
            .into_iter()
            .map(|i| i.specifier)
            .collect();
        // Same crate `use`d twice → one entry (deduped); crate/self/super are ignored.
        assert_eq!(specs, ["use:compass_core", "use:serde", "use:libc"]);
    }

    /// A two-crate Cargo workspace on disk, so `resolve` can read the crate map.
    fn workspace_ctx(current: &str, current_src: &str) -> MockResolutionContext {
        MockResolutionContext::new()
            .disk(
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/app\", \"crates/core\"]\n",
            )
            .disk("crates/core/Cargo.toml", "[package]\nname = \"my-core\"\n")
            .disk("crates/app/Cargo.toml", "[package]\nname = \"my-app\"\n")
            .file("crates/core/src/lib.rs")
            .file("crates/app/src/lib.rs")
            .current(current, current_src)
    }

    fn crate_use(name: &str) -> RawImport {
        raw(&format!("use:{name}"))
    }

    #[test]
    fn resolve_cross_crate_use_to_lib_root() {
        // `my-core` package → `my_core` import name → crates/core/src/lib.rs.
        let ctx = workspace_ctx("crates/app/src/main.rs", "use my_core::Thing;");
        let resolved = RustExtractor.resolve(&[crate_use("my_core")], &ctx, &LangConfig);
        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("crates/core/src/lib.rs"))
            }
            other => panic!("`use my_core::…` should resolve to core's lib.rs, got {other:?}"),
        }
    }

    #[test]
    fn external_crates_resolve_as_external_not_broken() {
        let ctx = workspace_ctx("crates/app/src/lib.rs", "");
        let resolved =
            RustExtractor.resolve(&[crate_use("std"), crate_use("serde")], &ctx, &LangConfig);
        assert!(
            resolved
                .iter()
                .all(|r| matches!(r, ResolvedImport::External { .. })),
            "std / third-party crates must be External (no edge, no diagnostic), got {resolved:?}"
        );
    }

    #[test]
    fn mod_resolution_still_works_alongside_use() {
        // A `mod` import in the same call must still resolve intra-crate.
        let ctx = workspace_ctx("crates/app/src/lib.rs", "").file("crates/app/src/util.rs");
        let resolved =
            RustExtractor.resolve(&[raw("util"), crate_use("my_core")], &ctx, &LangConfig);
        assert!(
            matches!(&resolved[0], ResolvedImport::Resolved { target, .. } if *target == ctx.id_of("crates/app/src/util.rs"))
        );
        assert!(
            matches!(&resolved[1], ResolvedImport::Resolved { target, .. } if *target == ctx.id_of("crates/core/src/lib.rs"))
        );
    }

    #[test]
    fn resolve_classifies_internal_external_and_broken() {
        // `src/lib.rs` is a module root, so its submodules resolve in the same dir (`src/`).
        let ctx = MockResolutionContext::new()
            .current("src/lib.rs", "")
            .file("src/util.rs")
            .file("src/parser/mod.rs");
        let imports = [raw("util"), raw("parser"), raw("missing")];
        let resolved = RustExtractor.resolve(&imports, &ctx, &LangConfig);

        // `mod util;` -> src/util.rs
        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/util.rs"))
            }
            other => panic!("`mod util;` should resolve to src/util.rs, got {other:?}"),
        }
        // `mod parser;` -> src/parser/mod.rs (the `foo/mod.rs` candidate)
        match &resolved[1] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/parser/mod.rs"))
            }
            other => panic!("`mod parser;` should resolve to src/parser/mod.rs, got {other:?}"),
        }
        // `mod missing;` -> no .rs file. Rust only ever produces Resolved or Unresolved for
        // `mod` declarations (they cannot point out-of-repo), so a missing one is Unresolved.
        assert!(
            matches!(resolved[2], ResolvedImport::Unresolved { .. }),
            "`mod missing;` resolves to no file, got {:?}",
            resolved[2]
        );
    }

    #[test]
    fn resolve_nests_submodules_of_a_non_root_file() {
        // A non-(lib/main/mod) file nests its submodules under `<stem>/`.
        let ctx = MockResolutionContext::new()
            .current("src/parser.rs", "")
            .file("src/parser/lexer.rs");
        let resolved = RustExtractor.resolve(&[raw("lexer")], &ctx, &LangConfig);
        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/parser/lexer.rs"))
            }
            other => panic!("`mod lexer;` should resolve under src/parser/, got {other:?}"),
        }
    }

    #[test]
    fn module_base_follows_2018_convention() {
        assert_eq!(module_base("src", "main"), "src");
        assert_eq!(module_base("src", "lib"), "src");
        assert_eq!(module_base("a/b", "mod"), "a/b");
        assert_eq!(module_base("src", "foo"), "src/foo");
    }
}
