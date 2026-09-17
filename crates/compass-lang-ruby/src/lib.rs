//! `compass-lang-ruby` — the Ruby language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.rb + shebang), the tree-sitter-ruby grammar, symbol extraction (classes,
//! modules, methods), plain-call capture, and `require_relative` resolution.
//!
//! In-repo file dependencies come from `require_relative` (resolved to a `.rb` file
//! relative to the current file). Plain `require` targets gems / the stdlib, so it is
//! treated as external and produces no edge.

use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawCall, RawImport,
    ResolutionContext, ResolvedImport,
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
        let mut calls = Vec::new();
        let mut visitor = Visitor {
            src: source,
            symbols: &mut symbols,
            imports: &mut imports,
            calls: &mut calls,
        };
        visitor.visit(tree.root_node(), &Scope::default());
        Extraction {
            symbols,
            imports,
            calls,
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
                    Some(target) => ResolvedImport::resolved(target, imp.span),
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

/// What a subtree is nested in. `in_class` is true directly inside a class-like body, where a
/// function is a method. `current_fn` is the `symbols` index of the enclosing function/method,
/// to which calls are attributed (`None` outside any).
#[derive(Default, Clone, Copy)]
struct Scope {
    in_class: bool,
    current_fn: Option<usize>,
}

/// Recursively pulls symbols, import specifiers and calls from the parse tree.
struct Visitor<'a> {
    src: &'a [u8],
    symbols: &'a mut Vec<ExtractedSymbol>,
    imports: &'a mut Vec<RawImport>,
    calls: &'a mut Vec<RawCall>,
}

impl Visitor<'_> {
    fn visit(&mut self, node: Node, scope: &Scope) {
        match node.kind() {
            "class" => {
                push_named(node, "name", SymbolKind::Class, self.src, self.symbols);
                self.recurse(
                    node,
                    &Scope {
                        in_class: true,
                        ..*scope
                    },
                );
                return;
            }
            "module" => {
                push_named(node, "name", SymbolKind::Module, self.src, self.symbols);
                self.recurse(
                    node,
                    &Scope {
                        in_class: true,
                        ..*scope
                    },
                );
                return;
            }
            "method" => {
                let kind = if scope.in_class {
                    SymbolKind::Method
                } else {
                    SymbolKind::Function
                };
                return self.enter_function(node, kind, scope);
            }
            "singleton_method" => return self.enter_function(node, SymbolKind::Method, scope),
            "call" => {
                let method = node.child_by_field_name("method");
                if method.and_then(|m| m.utf8_text(self.src).ok()) == Some("require_relative") {
                    if let Some(spec) = first_string_arg(node, self.src) {
                        self.imports.push(RawImport {
                            specifier: spec,
                            span: span_of(node),
                        });
                    }
                } else {
                    let callee = callee_name(node, self.src);
                    self.push_call(node, callee, scope);
                }
            }
            _ => {}
        }
        self.recurse(node, scope);
    }

    fn recurse(&mut self, node: Node, scope: &Scope) {
        let mut i = 0usize;
        while i < node.child_count() {
            if let Some(child) = node.child(i as u32) {
                self.visit(child, scope);
            }
            i += 1;
        }
    }

    fn push_call(&mut self, node: Node, callee: Option<String>, scope: &Scope) {
        if let (Some(caller), Some(callee)) = (scope.current_fn, callee) {
            self.calls.push(RawCall {
                caller,
                callee,
                span: span_of(node),
            });
        }
    }

    /// Push the function's symbol, then walk its body with that symbol as the caller (if it
    /// got a symbol at all — otherwise keep the parent as caller). A function body's own
    /// definitions are plain functions, not methods.
    fn enter_function(&mut self, node: Node, kind: SymbolKind, scope: &Scope) {
        let idx = self.symbols.len();
        push_named(node, "name", kind, self.src, self.symbols);
        let current_fn = if self.symbols.len() > idx {
            Some(idx)
        } else {
            scope.current_fn
        };
        self.recurse(
            node,
            &Scope {
                in_class: false,
                current_fn,
            },
        );
    }
}

/// The callee name when it can be named without type information: a receiver-less call
/// (`helper(1)`), a method on `self`, or `Const.new` (-> the class). A bare identifier without
/// arguments is not a `call` node at all (it may be a local variable), and calls on any other
/// receiver (`other.go`) need type resolution we don't do, so neither is guessed at.
fn callee_name(call: Node, src: &[u8]) -> Option<String> {
    let method = call.child_by_field_name("method")?;
    let name = match call.child_by_field_name("receiver") {
        None => method,
        Some(receiver) if receiver.kind() == "self" => method,
        Some(receiver) if receiver.kind() == "constant" && method.utf8_text(src) == Ok("new") => {
            receiver
        }
        Some(_) => return None,
    };
    if !matches!(name.kind(), "identifier" | "constant") {
        return None;
    }
    Some(name.utf8_text(src).ok()?.to_string())
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

    #[test]
    fn captures_calls_attributed_to_the_enclosing_function() {
        // Receiver-less calls, `self.` calls and `Const.new` are captured; calls on other
        // receivers (`other.prepare`) and top-level calls are not.
        let src = r#"
def helper(x)
  x
end

class Service
  def run(other)
    self.prepare
    helper(1)
    other.prepare
    Worker.new(2)
  end

  def prepare
  end
end

helper(0)
"#;
        let ex = extract(src);
        let mut got: Vec<(String, String)> = ex
            .calls
            .iter()
            .map(|c| (ex.symbols[c.caller].name.clone(), c.callee.clone()))
            .collect();
        got.sort();

        let want: Vec<(String, String)> =
            [("run", "Worker"), ("run", "helper"), ("run", "prepare")]
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect();
        assert_eq!(got, want);
    }
}
