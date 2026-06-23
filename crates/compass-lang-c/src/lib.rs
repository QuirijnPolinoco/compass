//! `compass-lang-c` — the C language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.c/.h), the tree-sitter-c grammar, symbol extraction (functions, structs,
//! unions, enums), and `#include` resolution.
//!
//! In-repo file dependencies come from quoted includes (`#include "foo.h"`), resolved
//! relative to the including file. Angle-bracket includes (`#include <stdio.h>`) are
//! system/library headers and produce no edge.

use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The C extractor. Registered by the CLI composition root (ADR-0003).
pub struct CExtractor;

impl Extractor for CExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("c")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["c", "h"],
            shebangs: &[],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_c::LANGUAGE.into()
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
        // Only count definitions (those with a body), not type references.
        "struct_specifier" | "union_specifier" => {
            if node.child_by_field_name("body").is_some() {
                push_named(node, "name", SymbolKind::Struct, src, symbols);
            }
        }
        "enum_specifier" => {
            if node.child_by_field_name("body").is_some() {
                push_named(node, "name", SymbolKind::Enum, src, symbols);
            }
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
                // `"foo.h"` is a local include; `<foo.h>` (system_lib_string) is external.
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

/// Descend a C declarator (through pointer/array/function wrappers) to its identifier.
fn declarator_name(mut node: Node, src: &[u8]) -> Option<String> {
    loop {
        match node.kind() {
            "identifier" => return node.utf8_text(src).ok().map(|s| s.to_string()),
            "function_declarator"
            | "pointer_declarator"
            | "array_declarator"
            | "parenthesized_declarator" => {
                node = node.child_by_field_name("declarator")?;
            }
            _ => return None,
        }
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
    use compass_core::SymbolKind::{Enum, Function, Struct};
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    // A rich sample exercising every emitting node kind plus decoys that must NOT
    // be extracted: quoted vs. system includes, struct/union/enum with and without
    // bodies, and functions whose names are nested under pointer/array declarators.
    const SAMPLE: &str = r#"
#include "util.h"
#include "missing.h"
#include <stdio.h>
#include <stdlib.h>

// Forward declaration (no body) — a type reference, NOT a definition.
struct Node;

// Enum used only as a return type below (no body here) — not re-extracted.
enum Color;

struct Point {
    int x;
    int y;
};

union Value {
    int i;
    float f;
};

enum Direction { NORTH, SOUTH, EAST, WEST };

int add(int a, int b) {
    return a + b;
}

// Return type `char *` forces descent through a pointer_declarator.
char *greet(void) {
    return "hi";
}

void noop(void) {
}

struct Point make_point(int x, int y) {
    struct Point p = { x, y };
    return p;
}
"#;

    fn extract(src: &str) -> Extraction {
        let c = CExtractor;
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
        // Only definitions with a body, plus every function (union -> Struct);
        // the bodyless `struct Node` / `enum Color` decoys are absent.
        let mut want = vec![
            ("Direction".to_string(), Enum),
            ("Point".to_string(), Struct),
            ("Value".to_string(), Struct),
            ("add".to_string(), Function),
            ("greet".to_string(), Function),
            ("make_point".to_string(), Function),
            ("noop".to_string(), Function),
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
        // Quoted includes only, in source order; `<stdio.h>`/`<stdlib.h>` are dropped.
        assert_eq!(specs, ["util.h", "missing.h"]);
    }

    #[test]
    fn resolve_classifies_internal_external_and_broken() {
        // C resolves quoted includes relative to the including file and treats a
        // not-found include as External (flexible `-I` dirs), never Unresolved.
        let ctx = MockResolutionContext::new()
            .current("src/main.c", "")
            .file("src/util.h");
        let imports = [
            raw("util.h"),    // resolves to src/util.h relative to src/main.c
            raw("missing.h"), // quoted but no mapped file -> External
        ];
        let resolved = CExtractor.resolve(&imports, &ctx, &LangConfig);

        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/util.h"))
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
        assert_eq!(resolve_path("src", "util.h"), "src/util.h");
        assert_eq!(resolve_path("src/a", "../util.h"), "src/util.h");
        assert_eq!(resolve_path("", "util.h"), "util.h");
        assert_eq!(resolve_path("src", "./util.h"), "src/util.h");
    }
}
