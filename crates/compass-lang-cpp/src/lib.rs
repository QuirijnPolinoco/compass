//! `compass-lang-cpp` — the C++ language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.cpp/.cc/.cxx/.hpp/.hh/.hxx), the tree-sitter-cpp grammar, symbol extraction
//! (functions, classes, structs, unions, enums, namespaces), and `#include` resolution.
//!
//! In-repo file dependencies come from quoted includes (`#include "foo.hpp"`), resolved
//! relative to the including file. Angle-bracket includes (`#include <vector>`) are
//! system/library headers and produce no edge. The C++ grammar is a superset of C, so the
//! quoted-include resolver mirrors `compass-lang-c`; a `.cpp` may include a `.h` owned by the
//! C extractor — the graph resolves it by path regardless of which extractor parsed the header.

use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The C++ extractor. Registered by the CLI composition root (ADR-0003).
pub struct CppExtractor;

impl Extractor for CppExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("cpp")
    }

    fn detection(&self) -> Detection {
        // C++-only extensions: `.c`/`.h` stay with the C extractor to avoid ambiguity.
        Detection {
            extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
            shebangs: &[],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_cpp::LANGUAGE.into()
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

        imports
            .iter()
            .map(|imp| {
                let candidate = resolve_path(&current_dir, &imp.specifier);
                match ctx.file_by_path(Path::new(&candidate)) {
                    Some(target) => ResolvedImport::resolved(target, imp.span),
                    // Include paths are flexible (`-I` dirs); don't flag broken.
                    None => ResolvedImport::External {
                        specifier: imp.specifier.clone(),
                    },
                }
            })
            .collect()
    }
}

/// Normalize a relative include path against the file's dir, collapsing `.`/`..`.
fn resolve_path(current_dir: &str, spec: &str) -> String {
    let mut parts: Vec<&str> = if current_dir.is_empty() {
        Vec::new()
    } else {
        current_dir.split('/').collect()
    };
    for seg in spec.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
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

fn visit(node: Node, src: &[u8], symbols: &mut Vec<ExtractedSymbol>, imports: &mut Vec<RawImport>) {
    match node.kind() {
        // Only count definitions (those with a body), not forward declarations / type references.
        "struct_specifier" | "union_specifier" => {
            if node.child_by_field_name("body").is_some() {
                push_named(node, "name", SymbolKind::Struct, src, symbols);
            }
        }
        "class_specifier" => {
            if node.child_by_field_name("body").is_some() {
                push_named(node, "name", SymbolKind::Class, src, symbols);
            }
        }
        "enum_specifier" => {
            if node.child_by_field_name("body").is_some() {
                push_named(node, "name", SymbolKind::Enum, src, symbols);
            }
        }
        // A named namespace is a module-like grouping; anonymous namespaces have no name field.
        "namespace_definition" => {
            push_named(node, "name", SymbolKind::Module, src, symbols);
        }
        "function_definition" => {
            if let Some(declarator) = node.child_by_field_name("declarator") {
                if let Some(name) = declarator_name(declarator, src) {
                    symbols.push(ExtractedSymbol {
                        name,
                        kind: SymbolKind::Function,
                        span: span_of(declarator),
                    });
                }
            }
        }
        "preproc_include" => {
            if let Some(path) = node.child_by_field_name("path") {
                // `"foo.hpp"` is a local include; `<vector>` (system_lib_string) is external.
                if path.kind() == "string_literal" {
                    if let Ok(text) = path.utf8_text(src) {
                        imports.push(RawImport {
                            specifier: text.trim_matches('"').to_string(),
                            span: span_of(node),
                        });
                    }
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

/// Descend a C++ declarator (through pointer/reference/array/function wrappers and the
/// `Class::method` qualifier) to the defined name.
fn declarator_name(node: Node, src: &[u8]) -> Option<String> {
    match node.kind() {
        // Leaf names: a plain identifier, a member name, `~Dtor`, or `operator+`.
        "identifier" | "field_identifier" | "destructor_name" | "operator_name" => {
            node.utf8_text(src).ok().map(|s| s.to_string())
        }
        // `Class::method` — the final unqualified name lives in the `name` field.
        "qualified_identifier" => node
            .child_by_field_name("name")
            .and_then(|n| declarator_name(n, src)),
        // These carry the inner declarator in a `declarator` field (inherited from C).
        "function_declarator"
        | "pointer_declarator"
        | "array_declarator"
        | "parenthesized_declarator" => node
            .child_by_field_name("declarator")
            .and_then(|n| declarator_name(n, src)),
        // `T& name` / `T&& name`: tree-sitter-cpp exposes no `declarator` field here, so
        // scan children for the inner declarator (the `&`/`&&` token yields nothing).
        "reference_declarator" => {
            let mut i = 0usize;
            while i < node.child_count() {
                if let Some(child) = node.child(i as u32) {
                    if let Some(name) = declarator_name(child, src) {
                        return Some(name);
                    }
                }
                i += 1;
            }
            None
        }
        _ => None,
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
    use compass_core::SymbolKind::{Class, Enum, Function, Module, Struct};
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    // A rich sample exercising every emitting node kind plus decoys that must NOT be
    // extracted: quoted vs. system includes, class/struct/enum with and without bodies,
    // a namespace, an out-of-line `Class::method` definition, and functions whose names
    // are nested under pointer/reference declarators.
    const SAMPLE: &str = r#"
#include "util.hpp"
#include "missing.hpp"
#include <vector>
#include <string>

// Forward declarations (no body) — type references, NOT definitions.
class Widget;
struct Bare;
enum Color;

namespace geo {

struct Point {
    int x;
    int y;
};

class Shape {
public:
    int sides() { return n; }
private:
    int n = 0;
};

enum Direction { NORTH, SOUTH, EAST, WEST };

union Value {
    int i;
    float f;
};

int add(int a, int b) {
    return a + b;
}

// Return type `char *` forces descent through a pointer_declarator.
char *greet() {
    return "hi";
}

// Return by reference forces descent through a reference_declarator.
int &pick(int &a) {
    return a;
}

}  // namespace geo

// Out-of-line member definition: name lives under a qualified_identifier.
int geo::Shape::area() {
    return 0;
}
"#;

    fn extract(src: &str) -> Extraction {
        let c = CppExtractor;
        let tree = compass_extract::parse(&c.grammar(), src.as_bytes()).expect("parse");
        c.extract(src.as_bytes(), &tree)
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
        // Definitions with a body (union -> Struct, class -> Class, namespace -> Module),
        // plus every function including the inline method and the out-of-line `area`.
        // The bodyless `Widget`/`Bare`/`Color` decoys are absent.
        let mut want = vec![
            ("Direction".to_string(), Enum),
            ("Point".to_string(), Struct),
            ("Shape".to_string(), Class),
            ("Value".to_string(), Struct),
            ("add".to_string(), Function),
            ("area".to_string(), Function),
            ("geo".to_string(), Module),
            ("greet".to_string(), Function),
            ("pick".to_string(), Function),
            ("sides".to_string(), Function),
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
        // Quoted includes only, in source order; `<vector>`/`<string>` are dropped.
        assert_eq!(specs, ["util.hpp", "missing.hpp"]);
    }

    #[test]
    fn resolve_classifies_internal_and_external() {
        // C++ resolves quoted includes relative to the including file and treats a
        // not-found include as External (flexible `-I` dirs), never Unresolved.
        let ctx = MockResolutionContext::new()
            .current("src/main.cpp", "")
            .file("src/util.hpp");
        let imports = [
            raw("util.hpp"),    // resolves to src/util.hpp relative to src/main.cpp
            raw("missing.hpp"), // quoted but no mapped file -> External
        ];
        let resolved = CppExtractor.resolve(&imports, &ctx, &LangConfig);

        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/util.hpp"))
            }
            other => panic!("local include should resolve, got {other:?}"),
        }
        assert!(
            matches!(resolved[1], ResolvedImport::External { .. }),
            "unmapped quoted include is External, not Unresolved"
        );
    }

    #[test]
    fn resolve_path_joins_relative_include() {
        assert_eq!(resolve_path("src", "util.hpp"), "src/util.hpp");
        assert_eq!(resolve_path("src/a", "../util.hpp"), "src/util.hpp");
        assert_eq!(resolve_path("", "util.hpp"), "util.hpp");
        assert_eq!(resolve_path("src", "./util.hpp"), "src/util.hpp");
    }
}
