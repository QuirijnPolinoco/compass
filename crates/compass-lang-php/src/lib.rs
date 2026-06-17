//! `compass-lang-php` — the PHP language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.php + shebang), the tree-sitter-php grammar, symbol extraction (classes,
//! interfaces, traits, enums, functions, methods), and `require`/`include` resolution.
//!
//! In-repo file dependencies come from `require`/`include` (+`_once`) with a literal path,
//! resolved relative to the importing file. `use` imports rely on PSR-4 autoloading
//! (a composer.json mapping), so they aren't resolved to files here — a planned enhancement.

use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The PHP extractor. Registered by the CLI composition root (ADR-0003).
pub struct PhpExtractor;

impl Extractor for PhpExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("php")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["php"],
            shebangs: &["php"],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_php::LANGUAGE_PHP.into()
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
                let base = resolve_path(&current_dir, &imp.specifier);
                let candidate = if base.ends_with(".php") {
                    base
                } else {
                    format!("{base}.php")
                };
                match ctx.file_by_path(Path::new(&candidate)) {
                    Some(target) => ResolvedImport::Resolved {
                        target,
                        span: imp.span,
                    },
                    // PHP include semantics are flexible (include_path); don't flag broken.
                    None => ResolvedImport::External {
                        specifier: imp.specifier.clone(),
                    },
                }
            })
            .collect()
    }
}

/// Normalize a relative path against the current file's dir, collapsing `.`/`..`.
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
        "class_declaration" => push_named(node, "name", SymbolKind::Class, src, symbols),
        "interface_declaration" => push_named(node, "name", SymbolKind::Interface, src, symbols),
        // A trait is a class-like unit of reusable methods (interfaces are separate in PHP).
        "trait_declaration" => push_named(node, "name", SymbolKind::Class, src, symbols),
        "enum_declaration" => push_named(node, "name", SymbolKind::Enum, src, symbols),
        "method_declaration" => push_named(node, "name", SymbolKind::Method, src, symbols),
        "function_definition" => push_named(node, "name", SymbolKind::Function, src, symbols),
        "require_expression"
        | "require_once_expression"
        | "include_expression"
        | "include_once_expression" => {
            // Only literal-path includes are resolvable (skip `__DIR__ . '...'` concat).
            if let Some(string) = first_child_of_kind(node, "string") {
                if let Ok(text) = string.utf8_text(src) {
                    imports.push(RawImport {
                        specifier: text.trim_matches(|c| c == '\'' || c == '"').to_string(),
                        span: span_of(node),
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

    const SAMPLE: &str = r#"<?php
namespace App;

use App\Util\Helper;

require_once 'util.php';

class Greeter {
    public function greet(): string {
        return "hi";
    }
}

interface Speaker {
    public function speak(): void;
}

trait Loud {}

function main(): void {
    echo (new Greeter())->greet();
}
"#;

    #[test]
    fn extracts_symbols_and_includes() {
        let php = PhpExtractor;
        let grammar = php.grammar();
        let tree = compass_extract::parse(&grammar, SAMPLE.as_bytes()).expect("parse");
        let extraction = php.extract(SAMPLE.as_bytes(), &tree);

        let by_name: Vec<(&str, SymbolKind)> = extraction
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert!(
            by_name.contains(&("Greeter", SymbolKind::Class)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Speaker", SymbolKind::Interface)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("greet", SymbolKind::Method)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("main", SymbolKind::Function)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Loud", SymbolKind::Class)),
            "{by_name:?}"
        );

        // Only literal include/require produces an import; `use` is skipped (PSR-4).
        let specs: Vec<&str> = extraction
            .imports
            .iter()
            .map(|i| i.specifier.as_str())
            .collect();
        assert_eq!(specs, vec!["util.php"], "{specs:?}");
    }

    #[test]
    fn resolve_path_collapses_segments() {
        assert_eq!(resolve_path("", "util.php"), "util.php");
        assert_eq!(resolve_path("a", "./inc/x.php"), "a/inc/x.php");
    }
}
