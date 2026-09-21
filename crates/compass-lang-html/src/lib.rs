//! `compass-lang-html` — the HTML language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.html/.htm), the tree-sitter-html grammar, symbol extraction (element ids), and
//! `href`/`src` resolution.
//!
//! HTML maps a different relationship than code does — *references*, not imports: a page
//! points at the stylesheets, scripts and other pages it uses (`<link href>`, `<script src>`,
//! `<a href>`, …). Every `href`/`src` that lands on a **mapped** file becomes an edge, so the
//! map shows which pages share a stylesheet and how pages link to each other. Element ids are
//! the symbols, written `#id` like the CSS extractor writes them, so `find_symbol("#main")`
//! finds both the element and the rules that style it.
//!
//! A reference is never reported as broken. Most `href`/`src` targets are legitimately absent
//! from the map — images, fonts, build output, routes served by a backend — and nothing in
//! the markup tells those apart from a typo, so a miss is treated as external.

use std::collections::HashSet;
use std::path::Path;

use compass_core::{FileCategory, LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The page a directory-style link (`docs/`) serves.
const DIRECTORY_INDEX: &str = "index.html";

/// The HTML extractor. Registered by the CLI composition root (ADR-0003).
pub struct HtmlExtractor;

impl Extractor for HtmlExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("html")
    }

    fn category(&self) -> FileCategory {
        FileCategory::markup()
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["html", "htm"],
            shebangs: &[],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_html::LANGUAGE.into()
    }

    fn extract(&self, source: &[u8], tree: &Tree) -> Extraction {
        let mut visitor = Visitor {
            src: source,
            symbols: Vec::new(),
            imports: Vec::new(),
            seen_ids: HashSet::new(),
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
                let external = || ResolvedImport::External {
                    specifier: imp.specifier.clone(),
                };
                let url = strip_query(&imp.specifier);
                if has_scheme(url) || url.starts_with("//") {
                    return external();
                }

                // `/css/x.css` is relative to a *site* root we can't see; the repo root is the
                // usual one, so a hit there is a convention-based (heuristic) edge.
                let (base, site_rooted) = match url.strip_prefix('/') {
                    Some(rest) => (resolve_path("", rest), true),
                    None => (resolve_path(&current_dir, url), false),
                };
                let target = ctx.file_by_path(Path::new(&base)).or_else(|| {
                    let index = resolve_path(&base, DIRECTORY_INDEX);
                    ctx.file_by_path(Path::new(&index))
                });
                match target {
                    Some(target) if site_rooted => ResolvedImport::heuristic(target, imp.span),
                    Some(target) => ResolvedImport::resolved(target, imp.span),
                    None => external(),
                }
            })
            .collect()
    }
}

/// `https:`, `mailto:`, `tel:`, `javascript:`, `data:` … — anything with a URL scheme.
fn has_scheme(url: &str) -> bool {
    let Some((scheme, _)) = url.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// Drop a `?query` / `#fragment` suffix from a URL.
fn strip_query(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// Template placeholders (`{{ url }}`, `{% static %}`, `<%= path %>`, `${base}`) are filled in
/// at render time, so the attribute's text is not a path.
fn is_templated(value: &str) -> bool {
    ["{{", "{%", "<%", "${"].iter().any(|t| value.contains(t))
}

/// Normalize a relative path against a directory, collapsing `.`/`..`.
fn resolve_path(dir: &str, spec: &str) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
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

/// Recursively pulls element ids and `href`/`src` references from the parse tree.
struct Visitor<'a> {
    src: &'a [u8],
    symbols: Vec<ExtractedSymbol>,
    imports: Vec<RawImport>,
    /// Ids already emitted: a (invalid but common) duplicate id is one symbol.
    seen_ids: HashSet<String>,
}

impl<'a> Visitor<'a> {
    fn visit(&mut self, node: Node) {
        if node.kind() == "attribute" {
            self.visit_attribute(node);
            return;
        }
        let mut i = 0usize;
        while i < node.child_count() {
            if let Some(child) = node.child(i as u32) {
                self.visit(child);
            }
            i += 1;
        }
    }

    fn visit_attribute(&mut self, attribute: Node) {
        let Some(name) = first_child_of_kind(attribute, "attribute_name") else {
            return;
        };
        let Some((value_node, value)) = self.attribute_value(attribute) else {
            return;
        };
        let value = value.trim();
        if value.is_empty() || is_templated(value) {
            return;
        }

        let name = self.text(name).unwrap_or_default();
        if name.eq_ignore_ascii_case("id") {
            if self.seen_ids.insert(value.to_string()) {
                self.symbols.push(ExtractedSymbol {
                    name: format!("#{value}"),
                    kind: SymbolKind::Other,
                    span: span_of(value_node),
                });
            }
        } else if name.eq_ignore_ascii_case("href") || name.eq_ignore_ascii_case("src") {
            // A pure `#fragment` points inside this page, not at another file.
            if !strip_query(value).is_empty() {
                self.imports.push(RawImport {
                    specifier: value.to_string(),
                    span: span_of(attribute),
                });
            }
        }
    }

    /// An attribute's value, quoted (`href="x"`) or bare (`src=x`); `None` for a boolean
    /// attribute (`defer`) or an empty one (`href=""`).
    fn attribute_value<'t>(&self, attribute: Node<'t>) -> Option<(Node<'t>, &'a str)> {
        let value = first_child_of_kind(attribute, "attribute_value").or_else(|| {
            let quoted = first_child_of_kind(attribute, "quoted_attribute_value")?;
            first_child_of_kind(quoted, "attribute_value")
        })?;
        Some((value, self.text(value)?))
    }

    fn text(&self, node: Node) -> Option<&'a str> {
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
    use compass_extract::testing::MockResolutionContext;

    // A sample exercising every extraction path:
    //  - ids -> `#id` symbols, each once (the duplicate `top` is dropped)
    //  - `href`/`src` on any element, quoted (single/double) or bare, case-insensitive name
    //  - skipped: a pure `#fragment`, an empty href, template placeholders, a boolean
    //    attribute, markup inside a comment, and inline <script>/<style> bodies (raw text)
    const SAMPLE: &str = r##"<!doctype html>
<html>
<head>
  <link rel="stylesheet" href="css/main.css?v=2">
  <script src=js/app.js defer></script>
  <script>import x from "./inline.js";</script>
  <style>@import "inline.css";</style>
</head>
<body id="top">
  <a HREF='about.html#team'>About</a>
  <img src="img/logo.png">
  <a href="#top">Back up</a>
  <a href="">Nowhere</a>
  <a href="{{ url_for('home') }}">Templated</a>
  <section id="team"></section>
  <div id="top"></div>
  <!-- <a href="commented-out.html">x</a> -->
</body>
</html>"##;

    fn extract(src: &str) -> Extraction {
        let x = HtmlExtractor;
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
    fn extracts_each_element_id_once() {
        let names: Vec<String> = extract(SAMPLE)
            .symbols
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["#top", "#team"]);
    }

    #[test]
    fn extracts_href_and_src_references_in_order() {
        let specs: Vec<String> = extract(SAMPLE)
            .imports
            .into_iter()
            .map(|i| i.specifier)
            .collect();
        assert_eq!(
            specs,
            [
                "css/main.css?v=2",
                "js/app.js",
                "about.html#team",
                "img/logo.png"
            ]
        );
    }

    #[test]
    fn resolve_links_to_mapped_files_and_treats_everything_else_as_external() {
        let ctx = MockResolutionContext::new()
            .current("site/index.html", "")
            .file("site/css/main.css")
            .file("site/about.html")
            .file("site/docs/index.html")
            .file("shared/app.js");

        let imports = [
            raw("css/main.css?v=2"), // query stripped
            raw("about.html#team"),  // fragment stripped
            raw("docs/"),            // directory -> its index.html
            raw("../shared/app.js"), // climbs out of site/
            raw("img/logo.png"),     // not a mapped file -> external, never broken
            raw("missing.html"),     // same: could be generated or served by a backend
            raw("https://example.com/"),
            raw("mailto:hi@example.com"),
            raw("//cdn.example/x.js"),
        ];
        let resolved = HtmlExtractor.resolve(&imports, &ctx, &LangConfig);

        for (r, want) in resolved.iter().zip([
            "site/css/main.css",
            "site/about.html",
            "site/docs/index.html",
            "shared/app.js",
        ]) {
            match r {
                ResolvedImport::Resolved { target, .. } => assert_eq!(*target, ctx.id_of(want)),
                other => panic!("expected Resolved({want}), got {other:?}"),
            }
        }
        for r in &resolved[4..] {
            assert!(matches!(r, ResolvedImport::External { .. }), "got {r:?}");
        }
    }

    #[test]
    fn a_site_rooted_link_resolves_from_the_repo_root_as_a_heuristic_edge() {
        use compass_core::EdgeConfidence;

        let ctx = MockResolutionContext::new()
            .current("pages/deep/page.html", "")
            .file("css/main.css");

        let resolved = HtmlExtractor.resolve(&[raw("/css/main.css")], &ctx, &LangConfig);
        match &resolved[0] {
            ResolvedImport::Resolved {
                target, confidence, ..
            } => {
                assert_eq!(*target, ctx.id_of("css/main.css"));
                assert_eq!(*confidence, EdgeConfidence::Heuristic);
            }
            other => panic!("expected a heuristic Resolved, got {other:?}"),
        }
    }

    #[test]
    fn has_scheme_recognises_urls_but_not_paths() {
        for url in [
            "https://x",
            "mailto:a@b",
            "tel:+1",
            "javascript:void(0)",
            "data:x",
        ] {
            assert!(has_scheme(url), "{url}");
        }
        for path in ["about.html", "a/b:c.html", "./x", "/root.css", "1:2"] {
            assert!(!has_scheme(path), "{path}");
        }
    }
}
