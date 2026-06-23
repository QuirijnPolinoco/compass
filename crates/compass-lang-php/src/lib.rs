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
    use compass_core::SymbolKind::{Class, Enum, Function, Interface, Method};
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    // A rich sample: every class-like kind PHP extracts (class, interface, trait, enum),
    // methods (including an abstract interface method) and a free function, plus several
    // include/require imports of different shapes. Only a single-quoted literal (a `string`
    // node) is extracted as an import: the `use` statement is skipped (PSR-4 autoload, not a
    // file dependency), a double-quoted `"..."` (an `encapsed_string`) is skipped, and the
    // `__DIR__ . '...'` concat include is skipped (the `string` is nested under a
    // `binary_expression`, not a direct child).
    const SAMPLE: &str = r#"<?php
namespace App;

use App\Util\Helper;

require_once 'util.php';
include 'lib/helpers.php';
require_once "skipped.php";
require __DIR__ . '/dynamic.php';

class Greeter {
    public function greet(): string {
        return "hi";
    }
}

interface Speaker {
    public function speak(): void;
}

trait Loud {}

enum Suit {
    case Hearts;
    case Spades;
}

function main(): void {
    echo (new Greeter())->greet();
}
"#;

    fn extract(src: &str) -> Extraction {
        let php = PhpExtractor;
        let tree = compass_extract::parse(&php.grammar(), src.as_bytes()).expect("parse");
        php.extract(src.as_bytes(), &tree)
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
        // A trait is extracted as Class (PHP interfaces are separate). The `enum` cases
        // are not declarations, so only `Suit` itself appears. The abstract interface
        // method `speak` is a `method_declaration`, so it is extracted like `greet`.
        let mut want = vec![
            ("Greeter".to_string(), Class),
            ("Loud".to_string(), Class),
            ("Speaker".to_string(), Interface),
            ("Suit".to_string(), Enum),
            ("greet".to_string(), Method),
            ("main".to_string(), Function),
            ("speak".to_string(), Method),
        ];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn extracts_all_imports_in_order() {
        // Only single-quoted literal include/require (+`_once`) produce imports, in source
        // order. `use` is skipped (PSR-4); the double-quoted `"skipped.php"` is an
        // `encapsed_string` (not a `string`) so it is skipped; and the `__DIR__ . '...'`
        // concat include is skipped (its `string` is nested under a `binary_expression`).
        let specs: Vec<String> = extract(SAMPLE)
            .imports
            .into_iter()
            .map(|i| i.specifier)
            .collect();
        assert_eq!(specs, ["util.php", "lib/helpers.php"]);
    }

    #[test]
    fn resolve_classifies_internal_and_external() {
        // The importing file is index.php at the repo root, so relative includes resolve
        // against the repo root. PHP never emits Unresolved: a missing include is External
        // (include_path semantics are too flexible to call it broken).
        let ctx = MockResolutionContext::new()
            .current("index.php", "<?php\n")
            .file("util.php")
            .file("lib/helpers.php");
        let imports = [
            raw("util.php"),         // resolves to util.php (already .php)
            raw("lib/helpers"),      // resolves to lib/helpers.php (.php appended)
            raw("missing/gone.php"), // not mapped -> External, not broken
        ];
        let resolved = PhpExtractor.resolve(&imports, &ctx, &LangConfig);

        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => assert_eq!(*target, ctx.id_of("util.php")),
            other => panic!("expected Resolved util.php, got {other:?}"),
        }
        match &resolved[1] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("lib/helpers.php"))
            }
            other => panic!("expected Resolved lib/helpers.php, got {other:?}"),
        }
        assert!(
            matches!(resolved[2], ResolvedImport::External { .. }),
            "missing include is External, not Unresolved: {:?}",
            resolved[2]
        );
    }

    #[test]
    fn resolve_path_collapses_segments() {
        assert_eq!(resolve_path("", "util.php"), "util.php");
        assert_eq!(resolve_path("a", "./inc/x.php"), "a/inc/x.php");
        assert_eq!(resolve_path("a/b", "../x.php"), "a/x.php");
    }

    #[test]
    fn parent_dir_drops_filename() {
        assert_eq!(parent_dir("src/index.php"), "src");
        assert_eq!(parent_dir("index.php"), "");
    }
}
