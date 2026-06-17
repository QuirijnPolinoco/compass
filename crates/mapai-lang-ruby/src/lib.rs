//! `mapai-lang-ruby` — the Ruby language extractor.
//!
//! A self-contained unit behind the [`mapai_extract::Extractor`] trait (ADR-0002):
//! detection (.rb + shebang), the tree-sitter-ruby grammar, symbol extraction (classes,
//! modules, methods), and `require_relative` resolution.
//!
//! In-repo file dependencies come from `require_relative` (resolved to a `.rb` file
//! relative to the current file). Plain `require` targets gems / the stdlib, so it is
//! treated as external and produces no edge.

use std::path::Path;

use mapai_core::{LanguageId, Span, SymbolKind};
use mapai_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The Ruby extractor. Registered by the CLI composition root (ADR-0003).
pub struct RubyExtractor;

impl Extractor for RubyExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("ruby")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["rb"],
            shebangs: &["ruby"],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_ruby::LANGUAGE.into()
    }

    fn extract(&self, source: &[u8], tree: &Tree) -> Extraction {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        visit(tree.root_node(), source, false, &mut symbols, &mut imports);
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
                let candidate = if base.ends_with(".rb") {
                    base
                } else {
                    format!("{base}.rb")
                };
                match ctx.file_by_path(Path::new(&candidate)) {
                    Some(target) => ResolvedImport::Resolved {
                        target,
                        span: imp.span,
                    },
                    None => ResolvedImport::Unresolved {
                        specifier: imp.specifier.clone(),
                        span: imp.span,
                        reason: "require_relative resolves to no .rb file".to_string(),
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

fn visit(
    node: Node,
    src: &[u8],
    in_class: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    imports: &mut Vec<RawImport>,
) {
    match node.kind() {
        "class" => {
            push_named(node, "name", SymbolKind::Class, src, symbols);
            recurse(node, src, true, symbols, imports);
            return;
        }
        "module" => {
            push_named(node, "name", SymbolKind::Module, src, symbols);
            recurse(node, src, true, symbols, imports);
            return;
        }
        "method" => {
            let kind = if in_class {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            push_named(node, "name", kind, src, symbols);
            recurse(node, src, false, symbols, imports);
            return;
        }
        "singleton_method" => {
            push_named(node, "name", SymbolKind::Method, src, symbols);
            recurse(node, src, false, symbols, imports);
            return;
        }
        "call" => {
            if let Some(method) = node.child_by_field_name("method") {
                if method.utf8_text(src) == Ok("require_relative") {
                    if let Some(spec) = first_string_arg(node, src) {
                        imports.push(RawImport {
                            specifier: spec,
                            span: span_of(node),
                        });
                    }
                }
            }
        }
        _ => {}
    }
    recurse(node, src, in_class, symbols, imports);
}

fn recurse(
    node: Node,
    src: &[u8],
    in_class: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    imports: &mut Vec<RawImport>,
) {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            visit(child, src, in_class, symbols, imports);
        }
        i += 1;
    }
}

/// The first string literal in a call's `arguments`, with quotes stripped.
fn first_string_arg(call: Node, src: &[u8]) -> Option<String> {
    let args = call.child_by_field_name("arguments")?;
    let string = first_child_of_kind(args, "string")?;
    let text = string.utf8_text(src).ok()?;
    Some(text.trim_matches(|c| c == '\'' || c == '"').to_string())
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

    const SAMPLE: &str = r#"
require 'json'
require_relative 'util'

module Greeting
  def self.hello
    "hi"
  end
end

class Greeter
  def greet
    "hello"
  end
end

def main
  puts Greeter.new.greet
end
"#;

    #[test]
    fn extracts_symbols_and_require_relative() {
        let rb = RubyExtractor;
        let grammar = rb.grammar();
        let tree = mapai_extract::parse(&grammar, SAMPLE.as_bytes()).expect("parse");
        let extraction = rb.extract(SAMPLE.as_bytes(), &tree);

        let by_name: Vec<(&str, SymbolKind)> = extraction
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert!(
            by_name.contains(&("Greeting", SymbolKind::Module)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Greeter", SymbolKind::Class)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("greet", SymbolKind::Method)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("hello", SymbolKind::Method)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("main", SymbolKind::Function)),
            "{by_name:?}"
        );

        // Only `require_relative` is captured (in-repo); plain `require` is external.
        let specs: Vec<&str> = extraction
            .imports
            .iter()
            .map(|i| i.specifier.as_str())
            .collect();
        assert_eq!(specs, vec!["util"], "{specs:?}");
    }

    #[test]
    fn resolve_path_collapses_segments() {
        assert_eq!(resolve_path("a/b", "util"), "a/b/util");
        assert_eq!(resolve_path("a/b", "../util"), "a/util");
    }
}
