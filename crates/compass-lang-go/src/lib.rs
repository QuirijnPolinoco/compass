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
        Extraction { symbols, imports }
    }

    fn resolve(
        &self,
        imports: &[RawImport],
        ctx: &dyn ResolutionContext,
        _config: &LangConfig,
    ) -> Vec<ResolvedImport> {
        // Go import paths are package paths, not relative file paths. An import that
        // begins with the module path (from go.mod) is internal and maps to a directory
        // of `.go` files; anything else is stdlib / third-party (external).
        let module_path = read_module_path(ctx.repo_root());
        let mut resolved = Vec::new();

        for imp in imports {
            let spec = imp.specifier.as_str();
            match module_path
                .as_deref()
                .and_then(|m| internal_subpath(m, spec))
            {
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

/// If `spec` is inside `module` (the go.mod module path), return the repo-relative
/// directory it maps to (e.g. module `example.com/demo`, import
/// `example.com/demo/util` -> `util`). Returns `None` for external packages.
fn internal_subpath(module: &str, spec: &str) -> Option<PathBuf> {
    if spec == module {
        return Some(PathBuf::new()); // the module root package
    }
    let rest = spec.strip_prefix(module)?.strip_prefix('/')?;
    Some(PathBuf::from(rest))
}

/// Read the `module` declaration from `<repo_root>/go.mod`, if present.
fn read_module_path(repo_root: &Path) -> Option<String> {
    let content = std::fs::read_to_string(repo_root.join("go.mod")).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix("module ") {
            return Some(rest.trim().to_string());
        }
    }
    None
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

    const SAMPLE: &str = r#"
package demo

import (
    "fmt"
    "example.com/demo/util"
)

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

func main() {
    util.Run()
}
"#;

    #[test]
    fn extracts_symbols_and_imports() {
        let go = GoExtractor;
        let grammar = go.grammar();
        let tree = compass_extract::parse(&grammar, SAMPLE.as_bytes()).expect("parse");
        let extraction = go.extract(SAMPLE.as_bytes(), &tree);

        let names: Vec<&str> = extraction.symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"Greeter"),
            "interface symbol, got {names:?}"
        );
        assert!(names.contains(&"Person"), "struct symbol, got {names:?}");
        assert!(names.contains(&"Greet"), "method symbol, got {names:?}");
        assert!(names.contains(&"main"), "function symbol, got {names:?}");

        let specs: Vec<&str> = extraction
            .imports
            .iter()
            .map(|i| i.specifier.as_str())
            .collect();
        assert!(specs.contains(&"fmt"), "stdlib import, got {specs:?}");
        assert!(
            specs.contains(&"example.com/demo/util"),
            "internal import, got {specs:?}"
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
