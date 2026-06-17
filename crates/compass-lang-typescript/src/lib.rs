//! `compass-lang-typescript` — the TypeScript/JavaScript language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.ts/.tsx/.js/.jsx/.mts/.cts/.mjs/.cjs), the tree-sitter TSX grammar (a
//! superset that parses TS, JS, and JSX), symbol extraction (functions, classes,
//! interfaces, enums, methods), and relative-import resolution.
//!
//! Bare specifiers (`react`) are external (node_modules). Relative imports that don't
//! resolve to a mapped file are treated as external too — a relative path may point at a
//! non-code asset (`./styles.css`), so we avoid false "broken import" diagnostics here.

use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// File extensions a relative import may resolve to, in priority order.
const SOURCE_EXTS: [&str; 8] = ["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"];

/// The TypeScript/JavaScript extractor. Registered by the CLI composition root (ADR-0003).
pub struct TypeScriptExtractor;

impl Extractor for TypeScriptExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("typescript")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"],
            shebangs: &["node"],
        }
    }

    fn grammar(&self) -> Language {
        // TSX is a superset that parses TS, plain JS, and JSX — one grammar for all.
        tree_sitter_typescript::LANGUAGE_TSX.into()
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
                let spec = imp.specifier.as_str();
                if !spec.starts_with('.') {
                    // Bare specifier → node_modules / built-in.
                    return ResolvedImport::External {
                        specifier: imp.specifier.clone(),
                    };
                }
                let base = resolve_path(&current_dir, spec);
                for cand in candidates(&base) {
                    if let Some(target) = ctx.file_by_path(Path::new(&cand)) {
                        return ResolvedImport::Resolved {
                            target,
                            span: imp.span,
                        };
                    }
                }
                // Unresolved relative import: likely a non-code asset; don't flag broken.
                ResolvedImport::External {
                    specifier: imp.specifier.clone(),
                }
            })
            .collect()
    }
}

/// Normalize a relative import against the importing file's directory, collapsing
/// `.`/`..` segments. e.g. (`a/b`, `../c/d`) -> `a/c/d`.
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

/// Candidate mapped files for a resolved base path (extensionless `./util` → `util.ts`,
/// `util/index.ts`, …), trying the base verbatim first in case it carried an extension.
fn candidates(base: &str) -> Vec<String> {
    let mut out = vec![base.to_string()];
    for ext in SOURCE_EXTS {
        out.push(format!("{base}.{ext}"));
    }
    for ext in ["ts", "tsx", "js", "jsx"] {
        out.push(format!("{base}/index.{ext}"));
    }
    out
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
        "function_declaration" | "generator_function_declaration" => {
            push_named(node, "name", SymbolKind::Function, src, symbols)
        }
        "class_declaration" | "abstract_class_declaration" => {
            push_named(node, "name", SymbolKind::Class, src, symbols)
        }
        "interface_declaration" => push_named(node, "name", SymbolKind::Interface, src, symbols),
        "enum_declaration" => push_named(node, "name", SymbolKind::Enum, src, symbols),
        "type_alias_declaration" => push_named(node, "name", SymbolKind::Other, src, symbols),
        "method_definition" => push_named(node, "name", SymbolKind::Method, src, symbols),
        "import_statement" | "export_statement" => {
            // The module specifier is the `source` string child (absent for re-exports
            // without `from`, and for plain `export { x }`).
            if let Some(string) = first_child_of_kind(node, "string") {
                if let Some(spec) = string_literal_text(string, src) {
                    imports.push(RawImport {
                        specifier: spec,
                        span: span_of(string),
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

fn first_child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            if child.kind() == kind {
                return Some(child);
            }
        }
        i += 1;
    }
    None
}

/// Read a `string` node's literal contents (strip the surrounding quotes).
fn string_literal_text(node: Node, src: &[u8]) -> Option<String> {
    let raw = node.utf8_text(src).ok()?;
    let trimmed = raw
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
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
import { Helper } from "./util";
import React from "react";
export { reexported } from "./other";

export function main() {
    return 1;
}

export class Service {
    run() {}
}

interface Greeter {
    greet(): string;
}

enum Color { Red, Green }
"#;

    #[test]
    fn extracts_symbols_and_imports() {
        let ts = TypeScriptExtractor;
        let grammar = ts.grammar();
        let tree = compass_extract::parse(&grammar, SAMPLE.as_bytes()).expect("parse");
        let extraction = ts.extract(SAMPLE.as_bytes(), &tree);

        let by_name: Vec<(&str, SymbolKind)> = extraction
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert!(
            by_name.contains(&("main", SymbolKind::Function)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Service", SymbolKind::Class)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("run", SymbolKind::Method)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Greeter", SymbolKind::Interface)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Color", SymbolKind::Enum)),
            "{by_name:?}"
        );

        let specs: Vec<&str> = extraction
            .imports
            .iter()
            .map(|i| i.specifier.as_str())
            .collect();
        assert!(specs.contains(&"./util"), "{specs:?}");
        assert!(specs.contains(&"react"), "{specs:?}");
        assert!(specs.contains(&"./other"), "{specs:?}");
    }

    #[test]
    fn resolve_path_collapses_segments() {
        assert_eq!(resolve_path("a/b", "./c"), "a/b/c");
        assert_eq!(resolve_path("a/b", "../c/d"), "a/c/d");
        assert_eq!(resolve_path("a/b", "../../x"), "x");
    }

    #[test]
    fn candidates_include_extensions_and_index() {
        let c = candidates("a/util");
        assert!(c.contains(&"a/util.ts".to_string()), "{c:?}");
        assert!(c.contains(&"a/util/index.ts".to_string()), "{c:?}");
    }
}
