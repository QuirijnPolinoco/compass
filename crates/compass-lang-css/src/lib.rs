//! `compass-lang-css` — the CSS language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.css), the tree-sitter-css grammar, symbol extraction (class and id selectors,
//! custom properties, keyframes), and `@import` resolution.
//!
//! In-repo file dependencies come from `@import "x.css"` / `@import url(x.css)`, resolved
//! relative to the importing stylesheet. CSS has no symbol→symbol calls. A stylesheet is a
//! leaf that HTML (and other stylesheets) point at, so the symbols are what it *offers*: each
//! distinct `.class`, `#id`, `--custom-property` and `@keyframes` name, once per file, written
//! with its sigil so `find_symbol(".card")` can't be confused with a code symbol named `card`.
//!
//! An `@import` that matches no file is only reported as broken when it is explicitly relative
//! (`./x.css`, `../x.css`). A bare `@import "normalize.css"` is as likely a bundler-resolved
//! package as a sibling file, so it is treated as external rather than guessed at.

use std::collections::HashSet;
use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The CSS extractor. Registered by the CLI composition root (ADR-0003).
pub struct CssExtractor;

impl Extractor for CssExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("css")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["css"],
            shebangs: &[],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_css::LANGUAGE.into()
    }

    fn extract(&self, source: &[u8], tree: &Tree) -> Extraction {
        let mut visitor = Visitor {
            src: source,
            symbols: Vec::new(),
            imports: Vec::new(),
            seen: HashSet::new(),
        };
        visitor.visit(tree.root_node());
        Extraction {
            symbols: visitor.symbols,
            imports: visitor.imports,
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
                let spec = imp.specifier.as_str();
                if is_remote(spec) {
                    return ResolvedImport::External {
                        specifier: imp.specifier.clone(),
                    };
                }
                let candidate = resolve_path(&current_dir, strip_query(spec));
                match ctx.file_by_path(Path::new(&candidate)) {
                    Some(target) => ResolvedImport::resolved(target, imp.span),
                    None if spec.starts_with("./") || spec.starts_with("../") => {
                        ResolvedImport::Unresolved {
                            specifier: imp.specifier.clone(),
                            span: imp.span,
                            reason: "relative @import matches no stylesheet".to_string(),
                        }
                    }
                    None => ResolvedImport::External {
                        specifier: imp.specifier.clone(),
                    },
                }
            })
            .collect()
    }
}

/// A URL that can never be an in-repo file: it has a scheme (`https:`, `data:`), is
/// protocol-relative (`//cdn…`), or is rooted at a site root we can't know (`/css/x.css`).
fn is_remote(spec: &str) -> bool {
    spec.contains("://") || spec.starts_with("data:") || spec.starts_with('/')
}

/// Drop a cache-busting `?v=3` / `#fragment` suffix from a URL.
fn strip_query(spec: &str) -> &str {
    spec.split(['?', '#']).next().unwrap_or(spec)
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

/// Recursively pulls symbols and `@import` specifiers from the parse tree.
struct Visitor<'a> {
    src: &'a [u8],
    symbols: Vec<ExtractedSymbol>,
    imports: Vec<RawImport>,
    /// Names already emitted: a selector repeated across rules is one symbol.
    seen: HashSet<String>,
}

impl Visitor<'_> {
    fn visit(&mut self, node: Node) {
        match node.kind() {
            "import_statement" => {
                if let Some(specifier) = self.import_target(node) {
                    self.imports.push(RawImport {
                        specifier,
                        span: span_of(node),
                    });
                }
                return;
            }
            // `class_name` also names pseudo-classes (`:hover`); only a `.class` is a symbol.
            "class_selector" => self.push_sigiled(node, "class_name", ".", SymbolKind::Other),
            "id_selector" => self.push_sigiled(node, "id_name", "#", SymbolKind::Other),
            "keyframes_statement" => {
                self.push_sigiled(node, "keyframes_name", "", SymbolKind::Other)
            }
            // A custom property *definition* (`--brand: red`), not a `var(--brand)` use.
            "declaration" => {
                if let Some(property) = first_child_of_kind(node, "property_name") {
                    if self.text(property).is_some_and(|p| p.starts_with("--")) {
                        self.push_symbol(property, "", SymbolKind::Variable);
                    }
                }
            }
            _ => {}
        }

        let mut i = 0usize;
        while i < node.child_count() {
            if let Some(child) = node.child(i as u32) {
                self.visit(child);
            }
            i += 1;
        }
    }

    /// The URL of an `@import`: `"x.css"`, `url("x.css")` or `url(x.css)`.
    fn import_target(&self, import: Node) -> Option<String> {
        let structured = first_child_of_kind(import, "string_value").or_else(|| {
            let url = first_child_of_kind(import, "call_expression")?;
            let args = first_child_of_kind(url, "arguments")?;
            first_child_of_kind(args, "string_value")
                .or_else(|| first_child_of_kind(args, "plain_value"))
        });
        let text = match structured {
            Some(value) => self.text(value)?,
            // The grammar can't parse an unquoted `url(../x.css)` (the leading dots derail
            // it), so read the URL straight out of the statement's text.
            None => {
                let statement = self.text(import)?;
                let after = &statement[statement.find("url(")? + "url(".len()..];
                &after[..after.find(')')?]
            }
        };
        let text = text.trim().trim_matches(|c| c == '"' || c == '\'');
        (!text.is_empty()).then(|| text.to_string())
    }

    fn push_sigiled(&mut self, node: Node, name_kind: &str, sigil: &str, kind: SymbolKind) {
        if let Some(name) = first_child_of_kind(node, name_kind) {
            self.push_symbol(name, sigil, kind);
        }
    }

    fn push_symbol(&mut self, name_node: Node, sigil: &str, kind: SymbolKind) {
        let Some(text) = self.text(name_node) else {
            return;
        };
        let name = format!("{sigil}{text}");
        if self.seen.insert(name.clone()) {
            self.symbols.push(ExtractedSymbol {
                name,
                kind,
                span: span_of(name_node),
            });
        }
    }

    fn text(&self, node: Node) -> Option<&str> {
        node.utf8_text(self.src).ok()
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
    use compass_core::SymbolKind::{Other, Variable};
    use compass_extract::testing::MockResolutionContext;

    // A sample exercising every extraction path:
    //  - `.class` and `#id` selectors, incl. compound (`#main.wide`), nested in a combinator,
    //    and inside `@media` — each distinct name ONCE (`.card` appears three times)
    //  - pseudo-classes (`:root`, `:hover`) are NOT symbols
    //  - a custom-property definition is a Variable; its `var(--brand)` use is not
    //  - `@keyframes` name
    //  - imports: string, `url("…")`, bare `url(…)`; `url()` in a declaration is not an import
    const SAMPLE: &str = r#"
@import "base.css";
@import url("theme/dark.css") screen;
@import url(../shared/reset.css);

:root { --brand: red; }
.card, .card > .title:hover { color: var(--brand); background: url("img/bg.png"); }
#main.wide { margin: 0 }
@media (min-width: 10px) { .card { display: grid } .grid { gap: 0 } }
@keyframes spin { from { opacity: 0 } }
"#;

    fn extract(src: &str) -> Extraction {
        let x = CssExtractor;
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

        // Each name once; no `root`/`hover` pseudo-classes.
        let mut want: Vec<(String, SymbolKind)> = vec![
            ("--brand".to_string(), Variable),
            (".card".to_string(), Other),
            (".title".to_string(), Other),
            ("#main".to_string(), Other),
            (".wide".to_string(), Other),
            (".grid".to_string(), Other),
            ("spin".to_string(), Other),
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
        assert_eq!(specs, ["base.css", "theme/dark.css", "../shared/reset.css"]);
    }

    #[test]
    fn resolve_is_relative_to_the_importing_stylesheet() {
        let ctx = MockResolutionContext::new()
            .current("styles/main.css", "")
            .file("styles/base.css")
            .file("styles/theme/dark.css")
            .file("shared/reset.css");

        let imports = [
            raw("base.css"),
            raw("theme/dark.css?v=3"), // cache-busting query is ignored
            raw("../shared/reset.css"),
        ];
        let resolved = CssExtractor.resolve(&imports, &ctx, &LangConfig);

        for (r, want) in resolved.iter().zip([
            "styles/base.css",
            "styles/theme/dark.css",
            "shared/reset.css",
        ]) {
            match r {
                ResolvedImport::Resolved { target, .. } => assert_eq!(*target, ctx.id_of(want)),
                other => panic!("expected Resolved({want}), got {other:?}"),
            }
        }
    }

    #[test]
    fn only_an_explicitly_relative_miss_is_reported_as_broken() {
        let ctx = MockResolutionContext::new().current("styles/main.css", "");
        let imports = [
            raw("./missing.css"), // explicitly relative -> broken
            raw("normalize.css"), // may be a bundler-resolved package
            raw("https://fonts.example/inter.css"),
            raw("//cdn.example/x.css"),
            raw("/css/site-root.css"), // site root is unknowable
        ];
        let resolved = CssExtractor.resolve(&imports, &ctx, &LangConfig);

        assert!(matches!(resolved[0], ResolvedImport::Unresolved { .. }));
        for r in &resolved[1..] {
            assert!(matches!(r, ResolvedImport::External { .. }), "got {r:?}");
        }
    }
}
