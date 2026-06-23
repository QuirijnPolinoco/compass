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
    use compass_core::SymbolKind::{Class, Enum, Interface, Method, Struct};
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    // A rich sample: a class with a constructor + two methods, an interface with one
    // method declaration, an enum, a struct, and a record (records map to Struct).
    // Imports cover a BCL namespace (`System`), an internal namespace (`Company.Util`),
    // an aliased using, and a third-party namespace. NB: the extractor captures the
    // first qualified_name/identifier under a using_directive, so for an aliased
    // `using Json = System.Text.Json;` it captures the alias identifier `Json`.
    const SAMPLE: &str = r#"
using System;
using Company.Util;
using Json = System.Text.Json;
using ThirdParty.Widgets;

namespace Company.App
{
    public class Program
    {
        public Program() {}
        public void Run() {}
        public static void Main(string[] args) {}
    }

    interface IGreeter
    {
        string Greet();
    }

    enum Color { Red, Green }

    struct Point { public int X; }

    record Pair(int A, int B);
}
"#;

    fn extract(src: &str) -> Extraction {
        let cs = CSharpExtractor;
        let tree = compass_extract::parse(&cs.grammar(), src.as_bytes()).expect("parse");
        cs.extract(src.as_bytes(), &tree)
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
            ("Color".to_string(), Enum),
            ("Greet".to_string(), Method), // interface method declaration
            ("IGreeter".to_string(), Interface),
            ("Main".to_string(), Method),
            ("Pair".to_string(), Struct), // record -> Struct
            ("Point".to_string(), Struct),
            ("Program".to_string(), Class),
            ("Program".to_string(), Method), // constructor -> Method (same name as class)
            ("Run".to_string(), Method),
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
        // Source order; the aliased using `Json = System.Text.Json` yields the alias
        // identifier `Json` (the first qualified_name/identifier under the directive).
        assert_eq!(
            specs,
            ["System", "Company.Util", "Json", "ThirdParty.Widgets",]
        );
    }

    #[test]
    fn resolve_classifies_internal_external_and_broken() {
        // The resolver re-reads the current file for its `namespace` line, derives the
        // source root by stripping `Company/App` from the file's dir, then maps each
        // `using A.B` onto `<root>/A/B`. C# treats unmapped namespaces as External
        // (BCL / NuGet), never as Unresolved — so there is no broken-import variant here.
        let ctx = MockResolutionContext::new()
            .current(
                "src/Company/App/Program.cs",
                "namespace Company.App\n{\n}\n",
            )
            .file("src/Company/Util/Helpers.cs");
        let imports = [
            raw("System"),       // BCL -> no `src/System` dir -> External
            raw("Company.Util"), // -> `src/Company/Util` (has a file) -> Resolved
            raw("Missing.Pkg"),  // no such dir -> External (not Unresolved)
        ];
        let resolved = CSharpExtractor.resolve(&imports, &ctx, &LangConfig);

        assert!(
            matches!(resolved[0], ResolvedImport::External { .. }),
            "System is a BCL namespace: {:?}",
            resolved[0]
        );
        match &resolved[1] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/Company/Util/Helpers.cs"))
            }
            other => panic!("internal `using Company.Util` should resolve, got {other:?}"),
        }
        assert!(
            matches!(resolved[2], ResolvedImport::External { .. }),
            "an unmapped namespace is External, never Unresolved: {:?}",
            resolved[2]
        );
    }

    #[test]
    fn source_root_strips_namespace() {
        assert_eq!(source_root("src/Company/App", "Company.App"), "src");
        assert_eq!(source_root("Company/App", "Company.App"), "");
        assert_eq!(source_root("foo", ""), "foo");
    }
}
