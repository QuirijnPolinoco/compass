//! `compass-lang-rust` — the Rust language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.rs), the tree-sitter-rust grammar, symbol extraction (functions, methods,
//! structs, enums, unions, traits, type aliases, consts, modules), and module resolution.
//!
//! File dependencies come from `mod foo;` declarations, which Rust resolves to a file
//! (`foo.rs` or `foo/mod.rs`) using the 2018 module convention. `use` paths reference the
//! module tree those `mod` declarations build, so they aren't resolved to files here.

use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The Rust extractor. Registered by the CLI composition root (ADR-0003).
pub struct RustExtractor;

impl Extractor for RustExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("rust")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["rs"],
            shebangs: &[],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_rust::LANGUAGE.into()
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
        let current = normalize(ctx.current_file());
        let dir = parent_dir(&current);
        let base = module_base(dir, file_stem(&current));

        imports
            .iter()
            .map(|imp| {
                let name = imp.specifier.as_str();
                let candidates = [
                    join(&base, &format!("{name}.rs")),
                    join(&base, &format!("{name}/mod.rs")),
                ];
                for cand in &candidates {
                    if let Some(target) = ctx.file_by_path(Path::new(cand)) {
                        return ResolvedImport::Resolved {
                            target,
                            span: imp.span,
                        };
                    }
                }
                ResolvedImport::Unresolved {
                    specifier: imp.specifier.clone(),
                    span: imp.span,
                    reason: "`mod` declaration resolves to no .rs file".to_string(),
                }
            })
            .collect()
    }
}

/// Where submodules of a file live (Rust 2018): a `mod`/`lib`/`main` file's submodules sit
/// in its own directory; any other file `x.rs` nests its submodules under `x/`.
fn module_base(dir: &str, stem: &str) -> String {
    if matches!(stem, "lib" | "main" | "mod") {
        dir.to_string()
    } else {
        join(dir, stem)
    }
}

fn join(root: &str, path: &str) -> String {
    if root.is_empty() {
        path.to_string()
    } else {
        format!("{root}/{path}")
    }
}

fn parent_dir(rel: &str) -> &str {
    rel.rsplit_once('/').map(|(d, _)| d).unwrap_or("")
}

fn file_stem(rel: &str) -> &str {
    let name = rel.rsplit_once('/').map(|(_, n)| n).unwrap_or(rel);
    name.strip_suffix(".rs").unwrap_or(name)
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn visit(
    node: Node,
    src: &[u8],
    in_impl: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    imports: &mut Vec<RawImport>,
) {
    match node.kind() {
        "function_item" => {
            let kind = if in_impl {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            push_named(node, "name", kind, src, symbols);
            recurse(node, src, false, symbols, imports);
            return;
        }
        "impl_item" => {
            recurse(node, src, true, symbols, imports);
            return;
        }
        "trait_item" => {
            push_named(node, "name", SymbolKind::Interface, src, symbols);
            recurse(node, src, true, symbols, imports);
            return;
        }
        "mod_item" => {
            push_named(node, "name", SymbolKind::Module, src, symbols);
            // `mod foo;` (no inline body) pulls in a file; `mod foo { .. }` does not.
            if !has_child_kind(node, "declaration_list") {
                if let Some(name) = node.child_by_field_name("name") {
                    if let Ok(text) = name.utf8_text(src) {
                        imports.push(RawImport {
                            specifier: text.to_string(),
                            span: span_of(name),
                        });
                    }
                }
            }
            recurse(node, src, false, symbols, imports);
            return;
        }
        "struct_item" | "union_item" => push_named(node, "name", SymbolKind::Struct, src, symbols),
        "enum_item" => push_named(node, "name", SymbolKind::Enum, src, symbols),
        "type_item" => push_named(node, "name", SymbolKind::Other, src, symbols),
        "const_item" | "static_item" => {
            push_named(node, "name", SymbolKind::Constant, src, symbols)
        }
        _ => {}
    }
    recurse(node, src, in_impl, symbols, imports);
}

fn recurse(
    node: Node,
    src: &[u8],
    in_impl: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    imports: &mut Vec<RawImport>,
) {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            visit(child, src, in_impl, symbols, imports);
        }
        i += 1;
    }
}

fn has_child_kind(node: Node, kind: &str) -> bool {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            if child.kind() == kind {
                return true;
            }
        }
        i += 1;
    }
    false
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
mod util;

pub mod inline {
    pub fn helper() {}
}

use std::collections::HashMap;

pub struct Point { x: i32 }

pub enum Color { Red, Green }

pub trait Greeter {
    fn greet(&self) -> String;
}

impl Greeter for Point {
    fn greet(&self) -> String { String::new() }
}

const MAX: u32 = 10;

fn main() {
    let _ = util::run();
}
"#;

    #[test]
    fn extracts_symbols_and_mod_imports() {
        let rust = RustExtractor;
        let grammar = rust.grammar();
        let tree = compass_extract::parse(&grammar, SAMPLE.as_bytes()).expect("parse");
        let extraction = rust.extract(SAMPLE.as_bytes(), &tree);

        let by_name: Vec<(&str, SymbolKind)> = extraction
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert!(
            by_name.contains(&("util", SymbolKind::Module)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Point", SymbolKind::Struct)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Color", SymbolKind::Enum)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Greeter", SymbolKind::Interface)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("greet", SymbolKind::Method)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("MAX", SymbolKind::Constant)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("main", SymbolKind::Function)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("helper", SymbolKind::Function)),
            "{by_name:?}"
        );

        let specs: Vec<&str> = extraction
            .imports
            .iter()
            .map(|i| i.specifier.as_str())
            .collect();
        // `mod util;` is a file import; the inline `mod inline { .. }` is not.
        assert_eq!(specs, vec!["util"], "{specs:?}");
    }

    #[test]
    fn module_base_follows_2018_convention() {
        assert_eq!(module_base("src", "main"), "src");
        assert_eq!(module_base("src", "lib"), "src");
        assert_eq!(module_base("a/b", "mod"), "a/b");
        assert_eq!(module_base("src", "foo"), "src/foo");
    }
}
