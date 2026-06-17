//! TEMPLATE for a new MapAI language extractor.
//!
//! Copy this crate to `crates/mapai-lang-<name>/`, rename it, and work through the TODOs
//! below. The whole job is implementing one trait — [`mapai_extract::Extractor`] — so a
//! new language never touches `mapai-core`, `mapai-engine`, or `mapai-mcp` (the North
//! Star, ADR-0002). See `CONTRIBUTING.md` §4 for the full Definition of Done, and the
//! existing `mapai-lang-go` / `mapai-lang-python` crates for worked examples.

use mapai_core::{LanguageId, Span, SymbolKind};
use mapai_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

pub struct TemplateExtractor;

impl Extractor for TemplateExtractor {
    fn language_id(&self) -> LanguageId {
        // TODO: the language's id, e.g. "rust".
        LanguageId::new("template")
    }

    fn detection(&self) -> Detection {
        Detection {
            // TODO: file extensions (without the dot) and any shebang interpreter hints.
            extensions: &[/* "rs" */],
            shebangs: &[],
        }
    }

    fn grammar(&self) -> Language {
        // TODO: return your tree-sitter grammar, e.g. `tree_sitter_rust::LANGUAGE.into()`.
        todo!("add a tree-sitter grammar crate and return its LANGUAGE")
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
        // TODO: map each raw import specifier to a real file using `ctx`:
        //   - `ctx.current_file()`            the importing file (repo-relative)
        //   - `ctx.repo_root()`               to read project config (go.mod, tsconfig, …)
        //   - `ctx.file_by_path(rel)`         exact repo-relative lookup
        //   - `ctx.files_in_dir(rel_dir)`     a package/directory of files
        // Return `Resolved` for in-repo targets, `Unresolved` for genuinely broken
        // internal imports (becomes a diagnostic), and `External` for stdlib/third-party.
        let _ = ctx;
        imports
            .iter()
            .map(|imp| ResolvedImport::External {
                specifier: imp.specifier.clone(),
            })
            .collect()
    }
}

/// Walk the parse tree and collect symbols + raw imports. Match on the grammar's node
/// kinds (run `tree-sitter parse` on a sample file, or read the existing extractors, to
/// learn the node names for your language).
fn visit(node: Node, src: &[u8], symbols: &mut Vec<ExtractedSymbol>, imports: &mut Vec<RawImport>) {
    match node.kind() {
        // TODO: e.g. "function_item" => push_named(node, "name", SymbolKind::Function, ...),
        //       e.g. "use_declaration" => collect the imported path into `imports`.
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

/// Helper: push a named declaration (the `field`-named child) as a symbol.
#[allow(dead_code)]
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

#[allow(dead_code)]
fn span_of(node: Node) -> Span {
    let start = node.start_position();
    Span {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_row: start.row,
        start_col: start.column,
    }
}

// TODO: add tests with fixtures inside this crate (see mapai-lang-go/src/lib.rs `tests`).
// `cargo test -p mapai-lang-<name>` should pass before you open a PR.
