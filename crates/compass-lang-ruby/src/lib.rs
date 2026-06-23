//! `compass-lang-ruby` — the Ruby language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.rb + shebang), the tree-sitter-ruby grammar, symbol extraction (classes,
//! modules, methods), and `require_relative` resolution.
//!
//! In-repo file dependencies come from `require_relative` (resolved to a `.rb` file
//! relative to the current file). Plain `require` targets gems / the stdlib, so it is
//! treated as external and produces no edge.

use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
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
    use compass_core::SymbolKind::{Class, Function, Method, Module};
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    // A rich sample exercising every extraction path:
    //  - `module` -> Module, `class` -> Class
    //  - `method` inside a class/module -> Method
    //  - `singleton_method` (`def self.x`) -> Method (in a module AND in a class)
    //  - top-level `method` (`def main`) -> Function
    //  - a constant assignment (`GREETING = ...`) is NOT a symbol and must be dropped
    //  - imports: plain `require` is dropped; only `require_relative` survives, in order;
    //    a relative subdir import and one that won't resolve are included.
    const SAMPLE: &str = r#"
require "json"
require_relative "util"
require_relative "helpers/text"
require_relative "missing"

module Greeting
  GREETING = "hi"

  def self.hello
    "hi"
  end

  def shout
    "HI"
  end
end

class Greeter
  def initialize(name)
    @name = name
  end

  def greet
    "hello"
  end

  def self.create
    new("anon")
  end
end

def main
  puts Greeter.new("x").greet
end
"#;

    fn extract(src: &str) -> Extraction {
        let x = RubyExtractor;
        let tree = compass_extract::parse(&x.grammar(), src.as_bytes()).expect("parse");
        x.extract(src.as_bytes(), &tree)
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

        let mut want: Vec<(String, SymbolKind)> = vec![
            ("Greeting".to_string(), Module),
            ("hello".to_string(), Method), // singleton_method in module
            ("shout".to_string(), Method), // method in module
            ("Greeter".to_string(), Class),
            ("initialize".to_string(), Method),
            ("greet".to_string(), Method),
            ("create".to_string(), Method), // singleton_method in class
            ("main".to_string(), Function), // top-level method
        ];
        want.sort();

        // EXACT set: the constant `GREETING` must NOT appear, nothing extra may.
        assert_eq!(got, want);
    }

    #[test]
    fn extracts_all_imports_in_order() {
        // Plain `require "json"` is dropped; only the three `require_relative`
        // specifiers survive, in source order.
        let specs: Vec<String> = extract(SAMPLE)
            .imports
            .into_iter()
            .map(|i| i.specifier)
            .collect();
        assert_eq!(specs, ["util", "helpers/text", "missing"]);
    }

    #[test]
    fn resolve_classifies_internal_external_and_broken() {
        // Ruby's resolver only classifies a `require_relative` as Resolved (a mapped
        // .rb file relative to the current file) or Unresolved (no such file). There is
        // no External branch — plain `require` never reaches resolve (it is dropped at
        // extract time), so every specifier here is in-repo-relative.
        let ctx = MockResolutionContext::new()
            .current("main.rb", "")
            .file("util.rb")
            .file("helpers/text.rb");

        let imports = [
            raw("util"),         // -> util.rb, Resolved
            raw("helpers/text"), // -> helpers/text.rb (subdir), Resolved
            raw("missing"),      // -> missing.rb, not mapped -> Unresolved
        ];
        let resolved = RubyExtractor.resolve(&imports, &ctx, &LangConfig);

        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("util.rb"))
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
        match &resolved[1] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("helpers/text.rb"))
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
        assert!(matches!(resolved[2], ResolvedImport::Unresolved { .. }));
    }

    #[test]
    fn resolve_uses_current_files_directory_and_collapses_dotdot() {
        // `require_relative` is relative to the *current file's* directory, and `..`
        // segments climb out of it. From lib/main.rb, "../shared/util" -> shared/util.rb.
        let ctx = MockResolutionContext::new()
            .current("lib/main.rb", "")
            .file("shared/util.rb");

        let resolved = RubyExtractor.resolve(&[raw("../shared/util")], &ctx, &LangConfig);

        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("shared/util.rb"))
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn resolve_accepts_an_explicit_rb_suffix_without_doubling_it() {
        // A specifier that already ends in `.rb` must not become `util.rb.rb`.
        let ctx = MockResolutionContext::new()
            .current("main.rb", "")
            .file("util.rb");

        let resolved = RubyExtractor.resolve(&[raw("util.rb")], &ctx, &LangConfig);

        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("util.rb"))
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn resolve_path_collapses_segments() {
        assert_eq!(resolve_path("a/b", "util"), "a/b/util");
        assert_eq!(resolve_path("a/b", "../util"), "a/util");
        assert_eq!(resolve_path("a/b", "./util"), "a/b/util");
        assert_eq!(resolve_path("", "util"), "util");
        assert_eq!(resolve_path("a", "../../util"), "util");
    }
}
