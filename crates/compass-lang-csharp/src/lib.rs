//! `compass-lang-csharp` — the C# language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection, the tree-sitter-c-sharp grammar, symbol extraction (classes, structs,
//! interfaces, enums, records, methods), and `using`/namespace import resolution.
//!
//! C# namespaces are not guaranteed to mirror folders, so resolution is a best-effort
//! convention heuristic (derive a root by stripping the file's namespace from its path,
//! then map `using A.B` to that directory). Imports that don't map are treated as
//! external (BCL / NuGet), never as broken — avoiding false diagnostics.

use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The C# extractor. Registered by the CLI composition root (ADR-0003).
pub struct CSharpExtractor;

impl Extractor for CSharpExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("csharp")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["cs"],
            shebangs: &[],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_c_sharp::LANGUAGE.into()
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
        let current_dir = parent_dir(&normalize(ctx.current_file()));
        let namespace = read_namespace(ctx.repo_root(), ctx.current_file()).unwrap_or_default();
        let root = source_root(&current_dir, &namespace);

        let mut resolved = Vec::new();
        for imp in imports {
            // A `using` imports a namespace ⇒ (by convention) a directory of .cs files.
            let dir = join(&root, &imp.specifier.replace('.', "/"));
            let files = ctx.files_in_dir(Path::new(&dir));
            if files.is_empty() {
                resolved.push(ResolvedImport::External {
                    specifier: imp.specifier.clone(),
                });
            } else {
                for target in files {
                    resolved.push(ResolvedImport::Resolved {
                        target,
                        span: imp.span,
                    });
                }
            }
        }
        resolved
    }
}

/// Strip the namespace path off the file's directory to find the source root, e.g.
/// dir `src/Company/App` + namespace `Company.App` -> `src`.
fn source_root(current_dir: &str, namespace: &str) -> String {
    let ns_path = namespace.replace('.', "/");
    if !ns_path.is_empty() && current_dir.ends_with(&ns_path) {
        current_dir[..current_dir.len() - ns_path.len()]
            .trim_end_matches('/')
            .to_string()
    } else {
        current_dir.to_string()
    }
}

fn join(root: &str, path: &str) -> String {
    if root.is_empty() {
        path.to_string()
    } else {
        format!("{root}/{path}")
    }
}

fn parent_dir(rel: &str) -> String {
    rel.rsplit_once('/')
        .map(|(d, _)| d)
        .unwrap_or("")
        .to_string()
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn read_namespace(repo_root: &Path, rel: &Path) -> Option<String> {
    let content = std::fs::read_to_string(repo_root.join(rel)).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix("namespace ") {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

fn visit(node: Node, src: &[u8], symbols: &mut Vec<ExtractedSymbol>, imports: &mut Vec<RawImport>) {
    match node.kind() {
        "class_declaration" => push_named(node, "name", SymbolKind::Class, src, symbols),
        "struct_declaration" => push_named(node, "name", SymbolKind::Struct, src, symbols),
        "interface_declaration" => push_named(node, "name", SymbolKind::Interface, src, symbols),
        "enum_declaration" => push_named(node, "name", SymbolKind::Enum, src, symbols),
        "record_declaration" => push_named(node, "name", SymbolKind::Struct, src, symbols),
        "method_declaration" | "constructor_declaration" => {
            push_named(node, "name", SymbolKind::Method, src, symbols)
        }
        "using_directive" => {
            // Take the first direct namespace name, skipping any `alias =` part.
            let mut i = 0usize;
            while i < node.child_count() {
                if let Some(child) = node.child(i as u32) {
                    if matches!(child.kind(), "qualified_name" | "identifier") {
                        if let Ok(text) = child.utf8_text(src) {
                            imports.push(RawImport {
                                specifier: text.to_string(),
                                span: span_of(node),
                            });
                        }
                        break;
                    }
                }
                i += 1;
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
using System;
using Company.Util;

namespace Company.App
{
    public class Program
    {
        public void Run() {}
        public static void Main(string[] args) {}
    }

    interface IGreeter
    {
        string Greet();
    }

    enum Color { Red, Green }

    struct Point { public int X; }
}
"#;

    #[test]
    fn extracts_symbols_and_imports() {
        let cs = CSharpExtractor;
        let grammar = cs.grammar();
        let tree = compass_extract::parse(&grammar, SAMPLE.as_bytes()).expect("parse");
        let extraction = cs.extract(SAMPLE.as_bytes(), &tree);

        let by_name: Vec<(&str, SymbolKind)> = extraction
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert!(
            by_name.contains(&("Program", SymbolKind::Class)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("IGreeter", SymbolKind::Interface)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Color", SymbolKind::Enum)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Point", SymbolKind::Struct)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Run", SymbolKind::Method)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Main", SymbolKind::Method)),
            "{by_name:?}"
        );

        let specs: Vec<&str> = extraction
            .imports
            .iter()
            .map(|i| i.specifier.as_str())
            .collect();
        assert!(specs.contains(&"System"), "{specs:?}");
        assert!(specs.contains(&"Company.Util"), "{specs:?}");
    }

    #[test]
    fn source_root_strips_namespace() {
        assert_eq!(source_root("src/Company/App", "Company.App"), "src");
        assert_eq!(source_root("Company/App", "Company.App"), "");
        assert_eq!(source_root("foo", ""), "foo");
    }
}
