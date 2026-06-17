//! `mapai-lang-c` — the C language extractor.
//!
//! A self-contained unit behind the [`mapai_extract::Extractor`] trait (ADR-0002):
//! detection (.c/.h), the tree-sitter-c grammar, symbol extraction (functions, structs,
//! unions, enums), and `#include` resolution.
//!
//! In-repo file dependencies come from quoted includes (`#include "foo.h"`), resolved
//! relative to the including file. Angle-bracket includes (`#include <stdio.h>`) are
//! system/library headers and produce no edge.

use std::path::Path;

use mapai_core::{LanguageId, Span, SymbolKind};
use mapai_extract::{
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
        Extraction { symbols, imports }
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
                    Some(target) => ResolvedImport::Resolved {
                        target,
                        span: imp.span,
                    },
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

    const SAMPLE: &str = r#"
#include "util.h"
#include <stdio.h>

struct Point {
    int x;
};

enum Color { RED, GREEN };

int add(int a, int b) {
    return a + b;
}

char *greet(void) {
    return "hi";
}
"#;

    #[test]
    fn extracts_symbols_and_local_includes() {
        let c = CExtractor;
        let grammar = c.grammar();
        let tree = mapai_extract::parse(&grammar, SAMPLE.as_bytes()).expect("parse");
        let extraction = c.extract(SAMPLE.as_bytes(), &tree);

        let by_name: Vec<(&str, SymbolKind)> = extraction
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert!(
            by_name.contains(&("Point", SymbolKind::Struct)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Color", SymbolKind::Enum)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("add", SymbolKind::Function)),
            "{by_name:?}"
        );
        // `char *greet` exercises descending through pointer_declarator.
        assert!(
            by_name.contains(&("greet", SymbolKind::Function)),
            "{by_name:?}"
        );

        // Quoted include only; the system header `<stdio.h>` is external.
        let specs: Vec<&str> = extraction
            .imports
            .iter()
            .map(|i| i.specifier.as_str())
            .collect();
        assert_eq!(specs, vec!["util.h"], "{specs:?}");
    }

    #[test]
    fn resolve_path_joins_relative_include() {
        assert_eq!(resolve_path("src", "util.h"), "src/util.h");
        assert_eq!(resolve_path("src/a", "../util.h"), "src/util.h");
    }
}
