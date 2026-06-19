//! `compass-lang-typescript` — the TypeScript/JavaScript language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.ts/.tsx/.js/.jsx/.mts/.cts/.mjs/.cjs), the tree-sitter TSX grammar (a
//! superset that parses TS, JS, and JSX), symbol extraction (functions, classes,
//! interfaces, enums, methods), and relative-import resolution.
//!
//! Bare specifiers (`react`) are external (node_modules) — unless a nearest-`tsconfig.json`
//! `paths`/`baseUrl` alias (e.g. `@/* -> ./src/*`) maps them to a real file. Relative imports
//! that don't resolve are treated as external too (a relative path may point at a non-code
//! asset like `./styles.css`), so we avoid false "broken import" diagnostics. TS-ESM `.js`
//! specifiers also resolve to their `.ts` source, and `require()` / dynamic `import()` are
//! captured alongside `import`/`export … from`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// File extensions a relative import may resolve to, in priority order.
const SOURCE_EXTS: [&str; 8] = ["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"];

/// The TypeScript/JavaScript extractor. Registered by the CLI composition root (ADR-0003).
pub struct TypeScriptExtractor;

impl Extractor for TypeScriptExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("typescript")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"],
            shebangs: &["node"],
        }
    }

    fn grammar(&self) -> Language {
        // TSX is a superset that parses TS, plain JS, and JSX — one grammar for all.
        tree_sitter_typescript::LANGUAGE_TSX.into()
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
        let tsconfig = tsconfig_for(ctx.repo_root(), &current_dir);

        imports
            .iter()
            .map(|imp| {
                let spec = imp.specifier.as_str();

                // Candidate base paths to try, in order. Relative specs resolve against the
                // importing file's dir; non-relative ones go through tsconfig path aliases.
                let bases: Vec<String> = if spec.starts_with('.') {
                    vec![resolve_path(&current_dir, spec)]
                } else if let Some(ts) = &tsconfig {
                    ts.resolve_alias(spec)
                } else {
                    Vec::new()
                };

                for base in &bases {
                    for cand in candidates(base) {
                        if let Some(target) = ctx.file_by_path(Path::new(&cand)) {
                            return ResolvedImport::Resolved {
                                target,
                                span: imp.span,
                            };
                        }
                    }
                }
                // Bare/unmatched, or a relative import with no mapped file (maybe a non-code
                // asset) → External, never a broken-import diagnostic.
                ResolvedImport::External {
                    specifier: imp.specifier.clone(),
                }
            })
            .collect()
    }
}

/// A `tsconfig.json`'s `paths`/`baseUrl` aliases, ready to resolve bare-looking specifiers.
struct TsPaths {
    /// Repo-relative dir that `paths` targets resolve against (`<tsconfig dir>/<baseUrl>`).
    base_dir: String,
    patterns: Vec<TsPattern>,
}
struct TsPattern {
    prefix: String,
    suffix: String,
    star: bool,
    targets: Vec<String>,
}

impl TsPaths {
    /// Candidate repo-relative base paths a non-relative `spec` maps to via `paths` aliases.
    fn resolve_alias(&self, spec: &str) -> Vec<String> {
        let mut out = Vec::new();
        for p in &self.patterns {
            if p.star {
                if spec.len() >= p.prefix.len() + p.suffix.len()
                    && spec.starts_with(&p.prefix)
                    && spec.ends_with(&p.suffix)
                {
                    let captured = &spec[p.prefix.len()..spec.len() - p.suffix.len()];
                    for t in &p.targets {
                        out.push(resolve_path(&self.base_dir, &t.replace('*', captured)));
                    }
                }
            } else if spec == p.prefix {
                for t in &p.targets {
                    out.push(resolve_path(&self.base_dir, t));
                }
            }
        }
        out
    }
}

/// The nearest `tsconfig.json` (walking up from `current_dir` to the repo root) parsed into
/// alias rules, cached per config-file path. `None` if there is none, or it has no `paths`.
fn tsconfig_for(repo_root: &Path, current_dir: &str) -> Option<Arc<TsPaths>> {
    let mut dir = current_dir.to_string();
    loop {
        let rel = if dir.is_empty() {
            "tsconfig.json".to_string()
        } else {
            format!("{dir}/tsconfig.json")
        };
        if repo_root.join(&rel).is_file() {
            if let Some(found) = parse_tsconfig_cached(repo_root.join(&rel), &dir) {
                return Some(found);
            }
        }
        if dir.is_empty() {
            return None;
        }
        dir = parent_dir(&dir);
    }
}

fn parse_tsconfig_cached(abs: PathBuf, dir: &str) -> Option<Arc<TsPaths>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<Arc<TsPaths>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = cache.lock().expect("tsconfig cache poisoned");
    if let Some(hit) = guard.get(&abs) {
        return hit.clone();
    }
    let parsed = std::fs::read_to_string(&abs)
        .ok()
        .and_then(|text| parse_tsconfig(&text, dir))
        .map(Arc::new);
    guard.insert(abs, parsed.clone());
    parsed
}

fn parse_tsconfig(text: &str, dir: &str) -> Option<TsPaths> {
    let value: serde_json::Value = serde_json::from_str(&strip_jsonc(text)).ok()?;
    let opts = value.get("compilerOptions")?;
    let base_url = opts.get("baseUrl").and_then(|b| b.as_str()).unwrap_or(".");
    let paths = opts.get("paths").and_then(|p| p.as_object())?;
    let base_dir = resolve_path(dir, base_url);

    let mut patterns = Vec::new();
    for (key, val) in paths {
        let targets: Vec<String> = val
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if targets.is_empty() {
            continue;
        }
        match key.find('*') {
            Some(i) => patterns.push(TsPattern {
                prefix: key[..i].to_string(),
                suffix: key[i + 1..].to_string(),
                star: true,
                targets,
            }),
            None => patterns.push(TsPattern {
                prefix: key.clone(),
                suffix: String::new(),
                star: false,
                targets,
            }),
        }
    }
    (!patterns.is_empty()).then_some(TsPaths { base_dir, patterns })
}

/// Strip `//` and `/* */` comments and trailing commas so a JSONC `tsconfig.json` parses as
/// JSON. Operates on bytes (UTF-8 multibyte sequences pass through untouched).
fn strip_jsonc(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let (mut i, mut in_str) = (0usize, false);
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == b'\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == b'"' {
                in_str = false;
            }
            i += 1;
        } else if c == b'"' {
            in_str = true;
            out.push(c);
            i += 1;
        } else if c == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    // Drop trailing commas (`,` before `}`/`]`), which JSON rejects but JSONC allows.
    let mut clean: Vec<u8> = Vec::with_capacity(out.len());
    let (mut j, mut in_s) = (0usize, false);
    while j < out.len() {
        let c = out[j];
        if in_s {
            clean.push(c);
            if c == b'\\' && j + 1 < out.len() {
                clean.push(out[j + 1]);
                j += 2;
                continue;
            }
            if c == b'"' {
                in_s = false;
            }
            j += 1;
        } else if c == b'"' {
            in_s = true;
            clean.push(c);
            j += 1;
        } else if c == b',' {
            let mut k = j + 1;
            while k < out.len() && (out[k] as char).is_ascii_whitespace() {
                k += 1;
            }
            if k < out.len() && (out[k] == b'}' || out[k] == b']') {
                j += 1; // skip the trailing comma
            } else {
                clean.push(c);
                j += 1;
            }
        } else {
            clean.push(c);
            j += 1;
        }
    }
    String::from_utf8_lossy(&clean).into_owned()
}

/// Normalize a relative import against the importing file's directory, collapsing
/// `.`/`..` segments. e.g. (`a/b`, `../c/d`) -> `a/c/d`.
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

/// Candidate mapped files for a resolved base path (extensionless `./util` → `util.ts`,
/// `util/index.ts`, …), trying the base verbatim first in case it carried an extension.
fn candidates(base: &str) -> Vec<String> {
    let mut out = vec![base.to_string()];
    // TS ESM: an import that ends in a JS extension usually refers to the TS *source*
    // (`./errors.js` → `errors.ts`), so try the source equivalents first.
    if let Some(stem) = base.strip_suffix(".js") {
        out.push(format!("{stem}.ts"));
        out.push(format!("{stem}.tsx"));
    } else if let Some(stem) = base.strip_suffix(".jsx") {
        out.push(format!("{stem}.tsx"));
    } else if let Some(stem) = base.strip_suffix(".mjs") {
        out.push(format!("{stem}.mts"));
    } else if let Some(stem) = base.strip_suffix(".cjs") {
        out.push(format!("{stem}.cts"));
    }
    for ext in SOURCE_EXTS {
        out.push(format!("{base}.{ext}"));
    }
    for ext in ["ts", "tsx", "js", "jsx"] {
        out.push(format!("{base}/index.{ext}"));
    }
    out
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
        "function_declaration" | "generator_function_declaration" => {
            push_named(node, "name", SymbolKind::Function, src, symbols)
        }
        "class_declaration" | "abstract_class_declaration" => {
            push_named(node, "name", SymbolKind::Class, src, symbols)
        }
        "interface_declaration" => push_named(node, "name", SymbolKind::Interface, src, symbols),
        "enum_declaration" => push_named(node, "name", SymbolKind::Enum, src, symbols),
        "type_alias_declaration" => push_named(node, "name", SymbolKind::Other, src, symbols),
        "method_definition" => push_named(node, "name", SymbolKind::Method, src, symbols),
        "import_statement" | "export_statement" => {
            // The module specifier is the `source` string child (absent for re-exports
            // without `from`, and for plain `export { x }`).
            if let Some(string) = first_child_of_kind(node, "string") {
                if let Some(spec) = string_literal_text(string, src) {
                    imports.push(RawImport {
                        specifier: spec,
                        span: span_of(string),
                    });
                }
            }
        }
        // `require("x")` (CommonJS) and dynamic `import("x")` — a call whose callee is the
        // `require` identifier or the `import` keyword, with a string first argument.
        "call_expression" => {
            if let Some(callee) = node.child_by_field_name("function") {
                let is_require = callee
                    .utf8_text(src)
                    .map(|t| t == "require")
                    .unwrap_or(false);
                if (is_require || callee.kind() == "import")
                    && node.child_by_field_name("arguments").is_some()
                {
                    let args = node.child_by_field_name("arguments").unwrap();
                    if let Some(string) = first_child_of_kind(args, "string") {
                        if let Some(spec) = string_literal_text(string, src) {
                            imports.push(RawImport {
                                specifier: spec,
                                span: span_of(string),
                            });
                        }
                    }
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

/// Read a `string` node's literal contents (strip the surrounding quotes).
fn string_literal_text(node: Node, src: &[u8]) -> Option<String> {
    let raw = node.utf8_text(src).ok()?;
    let trimmed = raw
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
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
    use compass_core::SymbolKind::{Class, Enum, Function, Interface, Method, Other};
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    /// A rich sample exercising every symbol kind the extractor recognizes
    /// (function + generator-function, class + abstract-class, interface, enum,
    /// type alias -> Other, method) and several import flavours (relative,
    /// bare/third-party, a re-export `from`, and a relative one that won't map).
    const SAMPLE: &str = r#"
import { Helper } from "./util";
import React from "react";
export { reexported } from "./other";
import missing from "./missing";

export function main(): number {
    return 1;
}

function* counter() {
    yield 1;
}

export class Service {
    run(): void {}
    stop(): void {}
}

abstract class Base {
    handle(): void {}
}

interface Greeter {
    greet(): string;
}

enum Color { Red, Green }

type Id = string;
"#;

    fn extract(src: &str) -> Extraction {
        let x = TypeScriptExtractor;
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
            ("main".to_string(), Function),
            ("counter".to_string(), Function), // generator_function_declaration
            ("Service".to_string(), Class),
            ("run".to_string(), Method),
            ("stop".to_string(), Method),
            ("Base".to_string(), Class), // abstract_class_declaration
            ("handle".to_string(), Method),
            ("Greeter".to_string(), Interface),
            ("Color".to_string(), Enum),
            ("Id".to_string(), Other), // type_alias_declaration
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
        assert_eq!(specs, ["./util", "react", "./other", "./missing"]);
    }

    #[test]
    fn resolve_classifies_internal_external_and_broken() {
        // `resolve()` uses `file_by_path` only; the importing file lives in `src/`.
        let ctx = MockResolutionContext::new()
            .current("src/main.ts", "")
            .file("src/util.ts"); // resolution target for `./util` (extension fallback)

        let imports = [
            raw("react"),     // [0] bare specifier -> External (node_modules)
            raw("./util"),    // [1] relative, maps to src/util.ts -> Resolved
            raw("./missing"), // [2] relative, no mapped file -> External (asset, not broken)
        ];
        let resolved = TypeScriptExtractor.resolve(&imports, &ctx, &LangConfig);

        assert!(
            matches!(resolved[0], ResolvedImport::External { .. }),
            "bare specifier should be External, got {:?}",
            resolved[0]
        );
        match &resolved[1] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/util.ts"))
            }
            other => panic!("expected Resolved for ./util, got {other:?}"),
        }
        // This language never emits Unresolved: an unmapped relative import is treated
        // as External (it may point at a non-code asset) rather than a broken import.
        assert!(
            matches!(resolved[2], ResolvedImport::External { .. }),
            "unmapped relative import should be External, got {:?}",
            resolved[2]
        );
    }

    #[test]
    fn resolve_uses_index_fallback() {
        // `./widgets` with no `widgets.ts` should still resolve to `widgets/index.ts`.
        let ctx = MockResolutionContext::new()
            .current("src/app.ts", "")
            .file("src/widgets/index.ts");
        let resolved = TypeScriptExtractor.resolve(&[raw("./widgets")], &ctx, &LangConfig);
        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/widgets/index.ts"))
            }
            other => panic!("expected index.ts fallback Resolved, got {other:?}"),
        }
    }

    #[test]
    fn resolve_path_collapses_segments() {
        assert_eq!(resolve_path("a/b", "./c"), "a/b/c");
        assert_eq!(resolve_path("a/b", "../c/d"), "a/c/d");
        assert_eq!(resolve_path("a/b", "../../x"), "x");
    }

    #[test]
    fn candidates_include_extensions_and_index() {
        let c = candidates("a/util");
        assert!(c.contains(&"a/util.ts".to_string()), "{c:?}");
        assert!(c.contains(&"a/util/index.ts".to_string()), "{c:?}");
    }

    #[test]
    fn resolves_js_specifier_to_ts_source() {
        // TS ESM: `./b.js` actually refers to `b.ts`.
        let ctx = MockResolutionContext::new()
            .current("src/a.ts", "")
            .file("src/b.ts");
        let resolved = TypeScriptExtractor.resolve(&[raw("./b.js")], &ctx, &LangConfig);
        assert!(
            matches!(&resolved[0], ResolvedImport::Resolved { target, .. } if *target == ctx.id_of("src/b.ts")),
            "got {:?}",
            resolved[0]
        );
    }

    #[test]
    fn captures_require_and_dynamic_import() {
        let src = r#"
const x = require("./cjs-mod");
function load() { return import("./dyn-mod"); }
"#;
        let specs: Vec<String> = extract(src)
            .imports
            .into_iter()
            .map(|i| i.specifier)
            .collect();
        assert!(specs.contains(&"./cjs-mod".to_string()), "{specs:?}");
        assert!(specs.contains(&"./dyn-mod".to_string()), "{specs:?}");
    }

    #[test]
    fn resolves_tsconfig_path_alias() {
        // `@/* -> ./src/*` from the nearest tsconfig.json (the Next.js-style alias).
        let ctx = MockResolutionContext::new()
            .disk(
                "frontend/tsconfig.json",
                r#"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["./src/*"] } } }"#,
            )
            .file("frontend/src/components/Button.tsx")
            .current("frontend/src/pages/home.tsx", "");
        let resolved =
            TypeScriptExtractor.resolve(&[raw("@/components/Button")], &ctx, &LangConfig);
        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("frontend/src/components/Button.tsx"))
            }
            other => panic!("expected @/ alias to resolve, got {other:?}"),
        }
        // A bare specifier with no alias match is still External.
        let ext = TypeScriptExtractor.resolve(&[raw("react")], &ctx, &LangConfig);
        assert!(matches!(ext[0], ResolvedImport::External { .. }));
    }

    #[test]
    fn strip_jsonc_handles_comments_and_trailing_commas() {
        let v: serde_json::Value = serde_json::from_str(&strip_jsonc(
            "{\n  // a comment\n  \"a\": 1, /* block */\n  \"b\": [1, 2,],\n}",
        ))
        .expect("parses after stripping");
        assert_eq!(v["a"], 1);
        assert_eq!(v["b"][1], 2);
    }
}
