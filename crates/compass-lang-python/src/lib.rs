//! `compass-lang-python` — the Python language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection, the tree-sitter-python grammar, symbol extraction (functions, classes,
//! methods), and Python's relative + absolute import resolution.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The Python extractor. Registered by the CLI composition root (ADR-0003).
pub struct PythonExtractor;

impl Extractor for PythonExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("python")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["py", "pyi"],
            shebangs: &["python"],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_python::LANGUAGE.into()
    }

    fn extract(&self, source: &[u8], tree: &Tree) -> Extraction {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        visit(tree.root_node(), source, false, &mut symbols, &mut imports);
        Extraction {
            symbols,
            imports,
            calls: Vec::new(),
        }
    }

    fn resolve(
        &self,
        imports: &[RawImport],
        ctx: &dyn ResolutionContext,
        _config: &LangConfig,
    ) -> Vec<ResolvedImport> {
        let current = normalize(ctx.current_file());
        let current_dir = current.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let roots = source_roots(ctx);

        imports
            .iter()
            .map(|imp| {
                let spec = imp.specifier.as_str();
                let relative = spec.starts_with('.');
                let candidates = if relative {
                    relative_candidates(current_dir, spec)
                } else {
                    absolute_candidates(spec, &roots)
                };

                for cand in &candidates {
                    if let Some(target) = ctx.file_by_path(Path::new(cand)) {
                        return ResolvedImport::resolved(target, imp.span);
                    }
                }

                if relative {
                    // Relative imports must resolve inside the repo; if not, it's broken.
                    ResolvedImport::Unresolved {
                        specifier: imp.specifier.clone(),
                        span: imp.span,
                        reason: "relative import resolves to no file in the repo".to_string(),
                    }
                } else {
                    // Absolute imports that aren't in-repo are stdlib / third-party.
                    ResolvedImport::External {
                        specifier: imp.specifier.clone(),
                    }
                }
            })
            .collect()
    }
}

/// Candidate repo-relative files for an absolute module path (`a.b.c`), tried under every
/// discovered source root. This is what makes the `src/` layout work: `import app.util`
/// resolves to `src/app/util.py` as well as `app/util.py`. Resolution is `file_by_path`-gated,
/// so an extra root only ever adds recall, never a wrong edge.
fn absolute_candidates(module: &str, roots: &[String]) -> Vec<String> {
    let base = module.replace('.', "/");
    let mut out = Vec::with_capacity(roots.len() * 2);
    for root in roots {
        let prefixed = if root.is_empty() {
            base.clone()
        } else {
            format!("{root}/{base}")
        };
        out.push(format!("{prefixed}.py"));
        out.push(format!("{prefixed}/__init__.py"));
    }
    out
}

/// A repo's Python source roots (repo-relative dirs that absolute imports are resolved under),
/// built once and cached per repo root (resolve runs per file, but the layout is constant).
fn source_roots(ctx: &dyn ResolutionContext) -> Arc<Vec<String>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Vec<String>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("python source-root cache poisoned");
    guard
        .entry(ctx.repo_root().to_path_buf())
        .or_insert_with(|| Arc::new(compute_source_roots(ctx)))
        .clone()
}

/// Discover source roots from the mapped file set — no hardcoded `src`. A source root is the
/// parent of a *top-level* package: for each directory holding an `__init__.py`, if its parent
/// is not itself a package, that parent holds the package and is a source root (`src/app/` →
/// root `src`; `app/` → root ``). The repo root is always included (flat modules and PEP 420
/// namespace packages live there).
fn compute_source_roots(ctx: &dyn ResolutionContext) -> Vec<String> {
    let package_dirs: HashSet<String> = ctx
        .all_files()
        .iter()
        .filter_map(|f| {
            let s = normalize(f);
            if let Some(dir) = s.strip_suffix("/__init__.py") {
                Some(dir.to_string())
            } else if s == "__init__.py" {
                Some(String::new())
            } else {
                None
            }
        })
        .collect();

    let mut roots: HashSet<String> = HashSet::new();
    roots.insert(String::new());
    for dir in &package_dirs {
        let parent = parent_dir(dir);
        if !package_dirs.contains(parent) {
            roots.insert(parent.to_string());
        }
    }
    let mut roots: Vec<String> = roots.into_iter().collect();
    // Deterministic candidate order; "" sorts first, so a flat-layout hit wins ties.
    roots.sort();
    roots
}

/// The parent directory of a `/`-separated repo-relative path (`""` for a top-level entry).
fn parent_dir(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

/// Candidate repo-relative files for a relative import (`.mod`, `..pkg.sub`, `.`).
fn relative_candidates(current_dir: &str, spec: &str) -> Vec<String> {
    let dots = spec.chars().take_while(|&c| c == '.').count();
    let rest = &spec[dots..];
    // 1 leading dot = current package; each extra dot goes one level up.
    let base = go_up(current_dir, dots.saturating_sub(1));
    let sub = rest.replace('.', "/");

    let joined = match (base.is_empty(), sub.is_empty()) {
        (true, _) => sub.clone(),
        (false, true) => base.clone(),
        (false, false) => format!("{base}/{sub}"),
    };

    if rest.is_empty() {
        if joined.is_empty() {
            vec!["__init__.py".to_string()]
        } else {
            vec![format!("{joined}/__init__.py")]
        }
    } else {
        vec![format!("{joined}.py"), format!("{joined}/__init__.py")]
    }
}

/// Drop `levels` trailing components from a `/`-separated directory path.
fn go_up(dir: &str, levels: usize) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for _ in 0..levels {
        parts.pop();
    }
    parts.join("/")
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Recursively pull symbols and imports. `in_class` makes a `def` a Method rather than a
/// Function.
fn visit(
    node: Node,
    src: &[u8],
    in_class: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    imports: &mut Vec<RawImport>,
) {
    match node.kind() {
        "function_definition" => {
            let kind = if in_class {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            push_named(node, "name", kind, src, symbols);
            // A function body's own defs are plain functions, not methods.
            recurse(node, src, false, symbols, imports);
            return;
        }
        "class_definition" => {
            push_named(node, "name", SymbolKind::Class, src, symbols);
            recurse(node, src, true, symbols, imports);
            return;
        }
        "import_statement" => extract_plain_imports(node, src, imports),
        "import_from_statement" => {
            if let Some(module) = node.child_by_field_name("module_name") {
                if let Ok(text) = module.utf8_text(src) {
                    imports.push(RawImport {
                        specifier: text.to_string(),
                        span: span_of(module),
                    });
                }
            }
        }
        _ => {}
    }
    recurse(node, src, in_class, symbols, imports);
}

fn recurse(
    node: Node,
    src: &[u8],
    in_class: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    imports: &mut Vec<RawImport>,
) {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            visit(child, src, in_class, symbols, imports);
        }
        i += 1;
    }
}

/// `import a.b`, `import a.b as c` — record each module's dotted path.
fn extract_plain_imports(node: Node, src: &[u8], imports: &mut Vec<RawImport>) {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            let name_node = match child.kind() {
                "dotted_name" => Some(child),
                "aliased_import" => child.child_by_field_name("name"),
                _ => None,
            };
            if let Some(n) = name_node {
                if let Ok(text) = n.utf8_text(src) {
                    imports.push(RawImport {
                        specifier: text.to_string(),
                        span: span_of(n),
                    });
                }
            }
        }
        i += 1;
    }
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
    use compass_core::SymbolKind::{Class, Function, Method};
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    /// A rich sample: top-level + nested functions, two classes with methods (one of
    /// which nests a plain function in its body), and imports of every shape the
    /// extractor records — plain, aliased, absolute-from, and relative-from.
    const SAMPLE: &str = r#"
import os
import a.b as ab
from pkg.mod_a import helper
from . import sibling
from .sib import z
from ..util import thing


class Greeter:
    def greet(self):
        def inner():
            return 1

        return inner()

    def shout(self):
        return "HI"


class Repository:
    def fetch(self):
        return None


def main():
    pass


def make_adder(n):
    def add(x):
        return x + n

    return add
"#;

    fn extract(src: &str) -> Extraction {
        let py = PythonExtractor;
        let tree = compass_extract::parse(&py.grammar(), src.as_bytes()).expect("parse");
        py.extract(src.as_bytes(), &tree)
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
            ("Greeter".to_string(), Class),
            ("Repository".to_string(), Class),
            // A `def` directly inside a class body is a Method.
            ("greet".to_string(), Method),
            ("shout".to_string(), Method),
            ("fetch".to_string(), Method),
            // A `def` nested in a method/function body is a plain Function.
            ("inner".to_string(), Function),
            ("add".to_string(), Function),
            // Top-level `def`s are Functions.
            ("main".to_string(), Function),
            ("make_adder".to_string(), Function),
        ];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn extracts_all_imports_in_order() {
        let specs: Vec<String> = extract(SAMPLE)
            .imports
            .into_iter()
            .map(|i| i.specifier)
            .collect();
        assert_eq!(specs, ["os", "a.b", "pkg.mod_a", ".", ".sib", "..util"]);
    }

    #[test]
    fn resolve_classifies_internal_external_and_broken() {
        // resolve() reads no module source — only `current_file()` (for the relative
        // base dir) and `file_by_path()` (the mapped resolution targets).
        let ctx = MockResolutionContext::new()
            .current("pkg/app.py", "")
            .file("pkg/mod_a.py") // absolute `pkg.mod_a` -> pkg/mod_a.py
            .file("pkg/__init__.py"); // relative `.` -> pkg/__init__.py
        let imports = [
            raw("os"),        // [0] absolute, not in repo  -> External (stdlib)
            raw("pkg.mod_a"), // [1] absolute, in repo      -> Resolved
            raw("."),         // [2] relative to current pkg -> Resolved (pkg/__init__.py)
            raw(".sib"),      // [3] relative, missing file  -> Unresolved
        ];
        let resolved = PythonExtractor.resolve(&imports, &ctx, &LangConfig);

        assert!(
            matches!(resolved[0], ResolvedImport::External { .. }),
            "stdlib/absolute-not-in-repo is External, got {:?}",
            resolved[0]
        );
        match &resolved[1] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("pkg/mod_a.py"))
            }
            other => panic!("absolute intra-repo import should resolve, got {other:?}"),
        }
        match &resolved[2] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("pkg/__init__.py"))
            }
            other => panic!("relative import to existing pkg should resolve, got {other:?}"),
        }
        assert!(
            matches!(resolved[3], ResolvedImport::Unresolved { .. }),
            "relative import resolving to no in-repo file is Unresolved, got {:?}",
            resolved[3]
        );
    }

    #[test]
    fn absolute_candidates_cover_module_and_package() {
        // Flat layout (single `""` root) — module file then package __init__.
        assert_eq!(
            absolute_candidates("pkg.mod_a", &["".to_string()]),
            vec![
                "pkg/mod_a.py".to_string(),
                "pkg/mod_a/__init__.py".to_string()
            ]
        );
        // With a `src` root too, the src-layout candidates are appended.
        assert_eq!(
            absolute_candidates("pkg.mod_a", &["".to_string(), "src".to_string()]),
            vec![
                "pkg/mod_a.py".to_string(),
                "pkg/mod_a/__init__.py".to_string(),
                "src/pkg/mod_a.py".to_string(),
                "src/pkg/mod_a/__init__.py".to_string(),
            ]
        );
    }

    #[test]
    fn resolves_absolute_imports_in_a_src_layout() {
        // src-layout: the package lives under `src/`, so `import app.util` must resolve to
        // `src/app/util.py` even though the import is written without the `src` prefix. The
        // `src` root is discovered from `src/app/__init__.py` — not hardcoded.
        let ctx = MockResolutionContext::new()
            .current("src/app/main.py", "")
            .file("src/app/__init__.py")
            .file("src/app/util.py");
        let resolved = PythonExtractor.resolve(&[raw("app.util")], &ctx, &LangConfig);
        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/app/util.py"))
            }
            other => panic!("src-layout absolute import should resolve, got {other:?}"),
        }
    }

    #[test]
    fn discovers_repo_root_and_src_as_source_roots() {
        // A repo with both a flat package (`flat/`) and a src-layout package (`src/app/`)
        // yields roots `""` and `src` (sorted), and nothing deeper.
        let ctx = MockResolutionContext::new()
            .current("flat/x.py", "")
            .file("flat/__init__.py")
            .file("src/app/__init__.py")
            .file("src/app/sub/__init__.py");
        assert_eq!(
            compute_source_roots(&ctx),
            vec!["".to_string(), "src".to_string()]
        );
    }

    #[test]
    fn relative_candidates_walk_up() {
        // From `app/sub/main.py`, `.mod` is a sibling in `app/sub`.
        assert_eq!(
            relative_candidates("app/sub", ".mod"),
            vec![
                "app/sub/mod.py".to_string(),
                "app/sub/mod/__init__.py".to_string()
            ]
        );
        // `..util` goes up one level to `app/util`.
        assert_eq!(
            relative_candidates("app/sub", "..util"),
            vec![
                "app/util.py".to_string(),
                "app/util/__init__.py".to_string()
            ]
        );
        // `from . import x` points at the current package's __init__.
        assert_eq!(
            relative_candidates("app/sub", "."),
            vec!["app/sub/__init__.py".to_string()]
        );
    }
}
