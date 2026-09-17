//! `compass-lang-go` — the Go language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait: detection, the
//! tree-sitter-go grammar, per-file symbol/import/plain-call extraction, and Go's whole-repo
//! import resolution. It depends only on `compass-extract` + `compass-core` (ADR-0002).

use std::path::{Path, PathBuf};

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawCall, RawImport,
    ResolutionContext, ResolvedImport,
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
        let mut calls = Vec::new();
        let mut visitor = Visitor {
            src: source,
            symbols: &mut symbols,
            imports: &mut imports,
            calls: &mut calls,
        };
        visitor.visit(tree.root_node(), &Scope::default());
        Extraction {
            symbols,
            imports,
            calls,
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
                            resolved.push(ResolvedImport::resolved(target, imp.span));
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

/// What a subtree is nested in. `current_fn` is the `symbols` index of the enclosing
/// function/method, to which calls are attributed (`None` at package scope). `receiver` is
/// the enclosing method's receiver name (`s` in `func (s *Svc) Run()`), so `s.prep()` can be
/// told apart from a call on an arbitrary value.
#[derive(Default)]
struct Scope {
    current_fn: Option<usize>,
    receiver: Option<String>,
}

/// Recursively pulls symbols, import specifiers and calls from the parse tree.
struct Visitor<'a> {
    src: &'a [u8],
    symbols: &'a mut Vec<ExtractedSymbol>,
    imports: &'a mut Vec<RawImport>,
    calls: &'a mut Vec<RawCall>,
}

impl Visitor<'_> {
    fn visit(&mut self, node: Node, scope: &Scope) {
        match node.kind() {
            "function_declaration" => {
                return self.enter_function(node, SymbolKind::Function, None, scope);
            }
            "method_declaration" => {
                return self.enter_function(
                    node,
                    SymbolKind::Method,
                    receiver_name(node, self.src),
                    scope,
                );
            }
            "type_spec" => {
                let kind = match node.child_by_field_name("type").map(|n| n.kind()) {
                    Some("struct_type") => SymbolKind::Struct,
                    Some("interface_type") => SymbolKind::Interface,
                    _ => SymbolKind::Other,
                };
                push_named(node, "name", kind, self.src, self.symbols);
            }
            "import_spec" => {
                if let Some(path) = node.child_by_field_name("path") {
                    if let Ok(text) = path.utf8_text(self.src) {
                        self.imports.push(RawImport {
                            specifier: text.trim_matches('"').to_string(),
                            span: span_of(path),
                        });
                    }
                }
            }
            "call_expression" => {
                let callee = node
                    .child_by_field_name("function")
                    .and_then(|f| callee_name(f, scope, self.src));
                self.push_call(node, callee, scope);
            }
            _ => {}
        }
        self.recurse(node, scope);
    }

    /// Push the function's symbol, then walk its body with that symbol as the caller (if it
    /// got a symbol at all — otherwise keep the parent as caller).
    fn enter_function(
        &mut self,
        node: Node,
        kind: SymbolKind,
        receiver: Option<String>,
        scope: &Scope,
    ) {
        let idx = self.symbols.len();
        push_named(node, "name", kind, self.src, self.symbols);
        let current_fn = if self.symbols.len() > idx {
            Some(idx)
        } else {
            scope.current_fn
        };
        self.recurse(
            node,
            &Scope {
                current_fn,
                receiver,
            },
        );
    }

    fn recurse(&mut self, node: Node, scope: &Scope) {
        let mut i = 0usize;
        while i < node.child_count() {
            if let Some(child) = node.child(i as u32) {
                self.visit(child, scope);
            }
            i += 1;
        }
    }

    fn push_call(&mut self, node: Node, callee: Option<String>, scope: &Scope) {
        if let (Some(caller), Some(callee)) = (scope.current_fn, callee) {
            self.calls.push(RawCall {
                caller,
                callee,
                span: span_of(node),
            });
        }
    }
}

/// The name bound to a method's receiver: `s` in `func (s *Svc) Run()`.
fn receiver_name(method: Node, src: &[u8]) -> Option<String> {
    let receiver = method.child_by_field_name("receiver")?;
    let mut i = 0usize;
    while i < receiver.child_count() {
        if let Some(param) = receiver.child(i as u32) {
            if let Some(name) = param.child_by_field_name("name") {
                return Some(name.utf8_text(src).ok()?.to_string());
            }
        }
        i += 1;
    }
    None
}

/// The callee name when it can be named without type information: a bare identifier
/// (`helper()`) or a method on the enclosing method's own receiver (`s.prep()`). Any other
/// selector (`fmt.Println()`, `x.Do()`) needs package/type resolution we don't do, so it's left
/// for the engine to never see rather than guessed at.
fn callee_name(function: Node, scope: &Scope, src: &[u8]) -> Option<String> {
    let name = match function.kind() {
        "identifier" => function,
        "selector_expression" => {
            let operand = function.child_by_field_name("operand")?;
            let receiver = scope.receiver.as_deref()?;
            if operand.kind() != "identifier" || operand.utf8_text(src) != Ok(receiver) {
                return None;
            }
            function.child_by_field_name("field")?
        }
        _ => return None,
    };
    Some(name.utf8_text(src).ok()?.to_string())
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

    #[test]
    fn captures_calls_attributed_to_the_enclosing_function() {
        // Plain `helper()` and receiver calls `s.prep()` are captured; `fmt.Println()` /
        // `other.Do()` (package or unknown receivers) and package-scope calls are not.
        let src = r#"
package m

var ready = helper()

func helper() int { return 1 }

func (s *Svc) Run(other *Svc) {
	s.prep()
	other.prep()
	fmt.Println(helper())
	go func() { s.stop() }()
}

func (s *Svc) prep() {}
func (s *Svc) stop() {}
"#;
        let ex = extract(src);
        let mut got: Vec<(String, String)> = ex
            .calls
            .iter()
            .map(|c| (ex.symbols[c.caller].name.clone(), c.callee.clone()))
            .collect();
        got.sort();

        let want: Vec<(String, String)> = [("Run", "helper"), ("Run", "prep"), ("Run", "stop")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        assert_eq!(got, want);
    }
}
