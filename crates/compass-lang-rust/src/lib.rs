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
    use compass_core::SymbolKind::{
        Constant, Enum, Function, Interface, Method, Module, Other, Struct,
    };
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    /// A sample exercising every symbol kind the extractor recognises plus both file-backed
    /// (`mod foo;`) and inline (`mod foo { .. }`) module declarations.
    const SAMPLE: &str = r#"
mod util;
mod missing;

pub mod inline {
    pub fn helper() {}
}

use std::collections::HashMap;

pub struct Point {
    x: i32,
}

pub union Bits {
    int: u32,
    float: f32,
}

pub enum Color {
    Red,
    Green,
}

pub type Pair = (i32, i32);

pub const MAX: u32 = 10;

static GREETING: &str = "hi";

pub trait Greeter {
    fn greet(&self) -> String;
}

impl Greeter for Point {
    fn greet(&self) -> String {
        String::new()
    }
}

impl Point {
    fn x(&self) -> i32 {
        self.x
    }
}

fn main() {
    let _ = util::run();
}
"#;

    fn extract(src: &str) -> Extraction {
        let x = RustExtractor;
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
        let mut want = vec![
            // `mod foo;` / `mod foo { .. }` declarations are Module symbols.
            ("util".to_string(), Module),
            ("missing".to_string(), Module),
            ("inline".to_string(), Module),
            // A `fn` inside an inline module is a free Function (not a Method).
            ("helper".to_string(), Function),
            // struct / union both map to Struct; enum to Enum.
            ("Point".to_string(), Struct),
            ("Bits".to_string(), Struct),
            ("Color".to_string(), Enum),
            // type alias -> Other.
            ("Pair".to_string(), Other),
            // const and static both map to Constant.
            ("MAX".to_string(), Constant),
            ("GREETING".to_string(), Constant),
            // trait -> Interface; its required fn is a Method (trait body counts as impl).
            ("Greeter".to_string(), Interface),
            ("greet".to_string(), Method),
            // an inherent-impl fn is also a Method.
            ("x".to_string(), Method),
            // a free top-level fn is a Function.
            ("main".to_string(), Function),
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
        // Only file-backed `mod foo;` declarations are imports, in source order; the inline
        // `mod inline { .. }` and the `use` statement are not.
        assert_eq!(specs, ["util", "missing"]);
    }

    #[test]
    fn resolve_classifies_internal_external_and_broken() {
        // `src/lib.rs` is a module root, so its submodules resolve in the same dir (`src/`).
        let ctx = MockResolutionContext::new()
            .current("src/lib.rs", "")
            .file("src/util.rs")
            .file("src/parser/mod.rs");
        let imports = [raw("util"), raw("parser"), raw("missing")];
        let resolved = RustExtractor.resolve(&imports, &ctx, &LangConfig);

        // `mod util;` -> src/util.rs
        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/util.rs"))
            }
            other => panic!("`mod util;` should resolve to src/util.rs, got {other:?}"),
        }
        // `mod parser;` -> src/parser/mod.rs (the `foo/mod.rs` candidate)
        match &resolved[1] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/parser/mod.rs"))
            }
            other => panic!("`mod parser;` should resolve to src/parser/mod.rs, got {other:?}"),
        }
        // `mod missing;` -> no .rs file. Rust only ever produces Resolved or Unresolved for
        // `mod` declarations (they cannot point out-of-repo), so a missing one is Unresolved.
        assert!(
            matches!(resolved[2], ResolvedImport::Unresolved { .. }),
            "`mod missing;` resolves to no file, got {:?}",
            resolved[2]
        );
    }

    #[test]
    fn resolve_nests_submodules_of_a_non_root_file() {
        // A non-(lib/main/mod) file nests its submodules under `<stem>/`.
        let ctx = MockResolutionContext::new()
            .current("src/parser.rs", "")
            .file("src/parser/lexer.rs");
        let resolved = RustExtractor.resolve(&[raw("lexer")], &ctx, &LangConfig);
        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/parser/lexer.rs"))
            }
            other => panic!("`mod lexer;` should resolve under src/parser/, got {other:?}"),
        }
    }

    #[test]
    fn module_base_follows_2018_convention() {
        assert_eq!(module_base("src", "main"), "src");
        assert_eq!(module_base("src", "lib"), "src");
        assert_eq!(module_base("a/b", "mod"), "a/b");
        assert_eq!(module_base("src", "foo"), "src/foo");
    }
}
