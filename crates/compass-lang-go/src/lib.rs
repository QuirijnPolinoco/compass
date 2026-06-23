//! `compass-lang-go` — the Go language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait: detection, the
//! tree-sitter-go grammar, per-file symbol/import extraction, and Go's whole-repo import
//! resolution. It depends only on `compass-extract` + `compass-core` (ADR-0002).

use std::path::{Path, PathBuf};

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The Go extractor. Registered by the CLI composition root (ADR-0003).
pub struct GoExtractor;

impl Extractor for GoExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("go")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["go"],
            shebangs: &[],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_go::LANGUAGE.into()
    }

    fn extract(&self, source: &[u8], tree: &Tree) -> Extraction {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        visit(tree.root_node(), source, &mut symbols, &mut imports);
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
        // Go import paths are package paths, not relative file paths. An import that begins
        // with the module path (from go.mod) — or with a `replace`d module mapped to a local
        // directory — is internal and maps to a directory of `.go` files; anything else is
        // stdlib / third-party (external).
        let go_mod = read_go_mod(ctx.repo_root());
        let mut resolved = Vec::new();

        for imp in imports {
            let spec = imp.specifier.as_str();
            match go_mod.internal_dir(spec) {
                Some(subdir) => {
                    let files = ctx.files_in_dir(&subdir);
                    if files.is_empty() {
                        resolved.push(ResolvedImport::Unresolved {
                            specifier: imp.specifier.clone(),
                            span: imp.span,
                            reason: "internal import resolves to no mapped Go files".to_string(),
                        });
                    } else {
                        // A Go import depends on every file in the target package.
                        for target in files {
                            resolved.push(ResolvedImport::Resolved {
                                target,
                                span: imp.span,
                            });
                        }
                    }
                }
                None => resolved.push(ResolvedImport::External {
                    specifier: imp.specifier.clone(),
                }),
            }
        }
        resolved
    }
}

/// If `spec` is inside `module` (a module path), return the repo-relative subpath it maps to
/// (e.g. module `example.com/demo`, import `example.com/demo/util` -> `util`). `None` if `spec`
/// is not under `module`.
fn internal_subpath(module: &str, spec: &str) -> Option<PathBuf> {
    if spec == module {
        return Some(PathBuf::new()); // the module root package
    }
    let rest = spec.strip_prefix(module)?.strip_prefix('/')?;
    Some(PathBuf::from(rest))
}

/// The parts of `go.mod` that map import paths to in-repo directories: the module path plus any
/// `replace … => ./local` directives (a filesystem replacement redirects an otherwise-external
/// module to local `.go` files — common in monorepos / multi-module workspaces).
struct GoMod {
    module: Option<String>,
    /// `(replaced module prefix, repo-relative local dir)` — only filesystem (`./`) targets.
    replaces: Vec<(String, String)>,
}

impl GoMod {
    /// The repo-relative directory an import path maps to, via the module path first, then any
    /// local `replace`. `None` ⇒ stdlib / third-party (external).
    fn internal_dir(&self, spec: &str) -> Option<PathBuf> {
        if let Some(module) = self.module.as_deref() {
            if let Some(sub) = internal_subpath(module, spec) {
                return Some(sub);
            }
        }
        for (old, local) in &self.replaces {
            if let Some(sub) = internal_subpath(old, spec) {
                return Some(if sub.as_os_str().is_empty() {
                    PathBuf::from(local)
                } else {
                    Path::new(local).join(sub)
                });
            }
        }
        None
    }
}

/// Read `<repo_root>/go.mod`: the `module` declaration and any local `replace` directives
/// (both single-line and `replace ( … )` block form). Comments and version constraints are
/// ignored. Best-effort: a missing/odd go.mod just yields an empty [`GoMod`].
fn read_go_mod(repo_root: &Path) -> GoMod {
    let mut go_mod = GoMod {
        module: None,
        replaces: Vec::new(),
    };
    let Ok(content) = std::fs::read_to_string(repo_root.join("go.mod")) else {
        return go_mod;
    };

    let mut in_replace_block = false;
    for raw in content.lines() {
        // Drop line comments, then trim.
        let line = raw.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("module ") {
            go_mod.module = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("replace") {
            let rest = rest.trim();
            if rest == "(" {
                in_replace_block = true;
            } else {
                parse_replace(rest, &mut go_mod.replaces);
            }
        } else if in_replace_block {
            if line == ")" {
                in_replace_block = false;
            } else {
                parse_replace(line, &mut go_mod.replaces);
            }
        }
    }
    go_mod
}

/// Parse one `OLD [version] => NEW [version]` replace entry, recording it only when `NEW` is a
/// filesystem path (`./…`), which is the only form that maps to in-repo files.
fn parse_replace(entry: &str, replaces: &mut Vec<(String, String)>) {
    let Some((lhs, rhs)) = entry.split_once("=>") else {
        return;
    };
    // First whitespace-separated token on each side is the module path / target (skip versions).
    let (Some(old), Some(target)) = (lhs.split_whitespace().next(), rhs.split_whitespace().next())
    else {
        return;
    };
    if let Some(local) = local_replacement_dir(target) {
        replaces.push((old.to_string(), local));
    }
}

/// A `replace` target's repo-relative directory, but only for a `./`-rooted local path (Go
/// requires filesystem replacements to be explicitly relative). `../…` / absolute targets point
/// outside the indexed tree, and a bare module path is just a rename — all yield `None`.
fn local_replacement_dir(target: &str) -> Option<String> {
    if target.starts_with("../") || target.starts_with('/') || target == ".." {
        return None;
    }
    let rel = target.strip_prefix("./").unwrap_or(target);
    if rel == "." || rel.is_empty() {
        return Some(String::new());
    }
    // Require an explicit local marker so `=> example.com/y` (a module rename) stays external.
    target
        .starts_with('.')
        .then(|| rel.trim_end_matches('/').to_string())
}

/// Recursively pull symbols and import specifiers from the parse tree.
fn visit(node: Node, src: &[u8], symbols: &mut Vec<ExtractedSymbol>, imports: &mut Vec<RawImport>) {
    match node.kind() {
        "function_declaration" => push_named(node, "name", SymbolKind::Function, src, symbols),
        "method_declaration" => push_named(node, "name", SymbolKind::Method, src, symbols),
        "type_spec" => {
            let kind = match node.child_by_field_name("type").map(|n| n.kind()) {
                Some("struct_type") => SymbolKind::Struct,
                Some("interface_type") => SymbolKind::Interface,
                _ => SymbolKind::Other,
            };
            push_named(node, "name", kind, src, symbols);
        }
        "import_spec" => {
            if let Some(path) = node.child_by_field_name("path") {
                if let Ok(text) = path.utf8_text(src) {
                    imports.push(RawImport {
                        specifier: text.trim_matches('"').to_string(),
                        span: span_of(path),
                    });
                }
            }
        }
        _ => {}
    }

    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            visit(child, src, symbols, imports);
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
    use compass_core::SymbolKind::{Function, Interface, Method, Other, Struct};
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    const SAMPLE: &str = r#"
package demo

import (
	"fmt"
	"example.com/demo/util"
	"github.com/x/y"
)

type ID int

type Greeter interface {
	Greet() string
}

type Person struct {
	Name string
}

const Version = "1.0"

func (p Person) Greet() string {
	return fmt.Sprintf("hi %s", p.Name)
}

func (p *Person) SetName(n string) {
	p.Name = n
}

func New() Person {
	return Person{}
}

func main() {
	util.Run()
}
"#;

    fn extract(src: &str) -> Extraction {
        let go = GoExtractor;
        let tree = compass_extract::parse(&go.grammar(), src.as_bytes()).expect("parse");
        go.extract(src.as_bytes(), &tree)
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
            ("Greet".to_string(), Method),
            ("Greeter".to_string(), Interface),
            ("ID".to_string(), Other),
            ("New".to_string(), Function),
            ("Person".to_string(), Struct),
            ("SetName".to_string(), Method),
            ("main".to_string(), Function),
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
        assert_eq!(specs, ["fmt", "example.com/demo/util", "github.com/x/y"]);
    }

    #[test]
    fn resolve_classifies_internal_external_and_broken() {
        let ctx = MockResolutionContext::new()
            .disk("go.mod", "module example.com/demo\n\ngo 1.22\n")
            .current("main.go", "package main\n")
            .file("util/util.go");
        let imports = [
            raw("fmt"),
            raw("example.com/demo/util"),
            raw("github.com/x/y"),
            raw("example.com/demo/missing"),
        ];
        let resolved = GoExtractor.resolve(&imports, &ctx, &LangConfig);

        assert!(
            matches!(resolved[0], ResolvedImport::External { .. }),
            "fmt is stdlib"
        );
        match &resolved[1] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("util/util.go"))
            }
            other => panic!("internal import should resolve, got {other:?}"),
        }
        assert!(
            matches!(resolved[2], ResolvedImport::External { .. }),
            "third-party module path"
        );
        assert!(
            matches!(resolved[3], ResolvedImport::Unresolved { .. }),
            "internal pkg with no files is broken"
        );
    }

    #[test]
    fn resolve_honors_local_replace_directive() {
        // A `replace` to a local dir makes an otherwise-third-party module path internal.
        // Block form here; the replaced module `example.com/lib` lives in `./vendored/lib`.
        let ctx = MockResolutionContext::new()
            .disk(
                "go.mod",
                "module example.com/demo\n\nrequire example.com/lib v1.2.3\n\nreplace (\n\texample.com/lib v1.2.3 => ./vendored/lib\n)\n",
            )
            .current("main.go", "package main\n")
            .file("vendored/lib/lib.go");
        let imports = [raw("example.com/lib"), raw("example.com/lib/sub")];
        let resolved = GoExtractor.resolve(&imports, &ctx, &LangConfig);

        // `example.com/lib` -> ./vendored/lib (has lib.go) -> Resolved.
        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("vendored/lib/lib.go"))
            }
            other => panic!("replaced module should resolve to local dir, got {other:?}"),
        }
        // A subpackage of a replaced module with no mapped files is Unresolved (internal).
        assert!(
            matches!(resolved[1], ResolvedImport::Unresolved { .. }),
            "replaced subpackage with no files is internal-but-broken, got {:?}",
            resolved[1]
        );
    }

    #[test]
    fn single_line_replace_maps_module_root() {
        // Single-line replace, target is the repo root (`.`); import == replaced module.
        let ctx = MockResolutionContext::new()
            .disk(
                "go.mod",
                "module example.com/demo\n\nreplace example.com/old => .\n",
            )
            .current("main.go", "package main\n")
            .file("root_pkg.go");
        let resolved = GoExtractor.resolve(&[raw("example.com/old")], &ctx, &LangConfig);
        // `=> .` maps to the repo root; assert root_pkg.go is among the resolved targets (the
        // mock also maps go.mod into the root, which the real walk never would).
        let targets: Vec<_> = resolved
            .iter()
            .filter_map(|r| match r {
                ResolvedImport::Resolved { target, .. } => Some(*target),
                _ => None,
            })
            .collect();
        assert!(
            targets.contains(&ctx.id_of("root_pkg.go")),
            "`=> .` should map to the repo root, got {resolved:?}"
        );
    }

    #[test]
    fn module_rename_replace_stays_external() {
        // `replace A => B` where B is a module path (not `./…`) is a rename, still external.
        let ctx = MockResolutionContext::new()
            .disk(
                "go.mod",
                "module example.com/demo\n\nreplace example.com/x => example.com/y v1.0.0\n",
            )
            .current("main.go", "package main\n");
        let resolved = GoExtractor.resolve(&[raw("example.com/x")], &ctx, &LangConfig);
        assert!(
            matches!(resolved[0], ResolvedImport::External { .. }),
            "a module-path replacement is not local, got {:?}",
            resolved[0]
        );
    }

    #[test]
    fn internal_subpath_splits_module_prefix() {
        assert_eq!(
            internal_subpath("example.com/demo", "example.com/demo/util"),
            Some(PathBuf::from("util"))
        );
        assert_eq!(internal_subpath("example.com/demo", "fmt"), None);
    }
}
