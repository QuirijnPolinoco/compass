//! `compass-lang-r` — the R language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.R/.r + `Rscript` shebang), the tree-sitter-r grammar, symbol extraction
//! (functions, R6/S4/Reference classes and their methods, S4 generics), plain-call capture,
//! and `source()` resolution.
//!
//! In-repo file dependencies come from `source("path.R")` / `sys.source(...)`. R resolves
//! that path against the *working directory*, which by convention is the project root but is
//! the script's own folder just as often — so the resolver tries the repo root first, then
//! the current file's directory. `source(here::here("R", "utils.R"))` is always root-relative.
//! `library()` / `require()` load installed packages, so they are external and produce no edge.

use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawCall, RawImport,
    ResolutionContext, ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// Specifier prefix marking a `here::here(...)` path: resolved against the repo root only.
const HERE_PREFIX: &str = "here:";

/// Calls that define a class named by their first string argument.
const CLASS_DEFINERS: &[&str] = &["R6Class", "setClass", "setRefClass"];

/// The R extractor. Registered by the CLI composition root (ADR-0003).
pub struct RExtractor;

impl Extractor for RExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("r")
    }

    fn detection(&self) -> Detection {
        Detection {
            // Detection is case-sensitive and `.R` is the conventional spelling.
            extensions: &["R", "r"],
            shebangs: &["Rscript"],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_r::LANGUAGE.into()
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
        visitor.visit(tree.root_node(), false, None);
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
                let (spec, root_only) = match imp.specifier.strip_prefix(HERE_PREFIX) {
                    Some(rest) => (rest, true),
                    None => (imp.specifier.as_str(), false),
                };
                if is_external(spec) {
                    return ResolvedImport::External {
                        specifier: imp.specifier.clone(),
                    };
                }

                let spec = spec.replace('\\', "/");
                let mut candidates = vec![resolve_path("", &spec)];
                if !root_only && !current_dir.is_empty() {
                    candidates.push(resolve_path(&current_dir, &spec));
                }
                match candidates
                    .iter()
                    .find_map(|c| ctx.file_by_path(Path::new(c)))
                {
                    Some(target) => ResolvedImport::resolved(target, imp.span),
                    None => ResolvedImport::Unresolved {
                        specifier: imp.specifier.clone(),
                        span: imp.span,
                        reason: "source() path matches no file from the repo root or the \
                                 sourcing file's directory"
                            .to_string(),
                    },
                }
            })
            .collect()
    }
}

/// A `source()` target that can never be an in-repo file: a URL, an absolute path, or `~`.
fn is_external(spec: &str) -> bool {
    let drive_letter = spec.as_bytes().get(1) == Some(&b':');
    spec.contains("://") || spec.starts_with('/') || spec.starts_with('~') || drive_letter
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

struct Visitor<'a> {
    src: &'a [u8],
    symbols: &'a mut Vec<ExtractedSymbol>,
    imports: &'a mut Vec<RawImport>,
    calls: &'a mut Vec<RawCall>,
}

impl Visitor<'_> {
    /// `in_class` is true directly inside a class-defining call (`R6Class(...)`), where a
    /// `name = function(...)` argument is a method. `current_fn` is the `symbols` index of the
    /// enclosing function, to which calls are attributed (`None` at script scope).
    fn visit(&mut self, node: Node, in_class: bool, current_fn: Option<usize>) {
        match node.kind() {
            // `name <- function(...)`, `name = function(...)`, `name <<- function(...)`.
            "binary_operator" => {
                if let Some(func) = self.assigned_function(node) {
                    let idx = self.symbols.len();
                    if let Some(lhs) = node.child_by_field_name("lhs") {
                        self.push_symbol(lhs, SymbolKind::Function);
                    }
                    let caller = (self.symbols.len() > idx).then_some(idx).or(current_fn);
                    self.recurse(func, false, caller);
                    return;
                }
            }
            // `greet = function(...)` inside `R6Class(public = list(...))` -> a method.
            "argument" if in_class => {
                let value = node.child_by_field_name("value");
                if let Some(func) = value.filter(|v| v.kind() == "function_definition") {
                    let idx = self.symbols.len();
                    if let Some(name) = node.child_by_field_name("name") {
                        self.push_symbol(name, SymbolKind::Method);
                    }
                    let caller = (self.symbols.len() > idx).then_some(idx).or(current_fn);
                    self.recurse(func, false, caller);
                    return;
                }
            }
            // An anonymous function: whatever it contains is no longer class-level.
            "function_definition" => {
                self.recurse(node, false, current_fn);
                return;
            }
            "call" => {
                if let Some(name) = self.callee_name(node) {
                    match name.as_str() {
                        "source" | "sys.source" => {
                            if let Some(specifier) = self.source_target(node) {
                                self.imports.push(RawImport {
                                    specifier,
                                    span: span_of(node),
                                });
                            }
                        }
                        "setGeneric" => self.push_string_named(node, SymbolKind::Function),
                        n if CLASS_DEFINERS.contains(&n) => {
                            self.push_string_named(node, SymbolKind::Class);
                            self.recurse(node, true, current_fn);
                            return;
                        }
                        _ => {}
                    }
                    // Only *plain* `foo(..)` calls are attributed; `pkg::foo()` and `obj$foo()`
                    // need package/receiver resolution we don't do.
                    if let (Some(caller), true) = (current_fn, self.is_plain_call(node)) {
                        self.calls.push(RawCall {
                            caller,
                            callee: name,
                            span: span_of(node),
                        });
                    }
                }
            }
            _ => {}
        }
        self.recurse(node, in_class, current_fn);
    }

    fn recurse(&mut self, node: Node, in_class: bool, current_fn: Option<usize>) {
        let mut i = 0usize;
        while i < node.child_count() {
            if let Some(child) = node.child(i as u32) {
                self.visit(child, in_class, current_fn);
            }
            i += 1;
        }
    }

    /// The `function_definition` on the right of an assignment to a plain name, if this
    /// `binary_operator` is one.
    fn assigned_function<'t>(&self, node: Node<'t>) -> Option<Node<'t>> {
        let op = node
            .child_by_field_name("operator")?
            .utf8_text(self.src)
            .ok()?;
        if !matches!(op, "<-" | "<<-" | "=") {
            return None;
        }
        let lhs = node.child_by_field_name("lhs")?;
        let rhs = node.child_by_field_name("rhs")?;
        (lhs.kind() == "identifier" && rhs.kind() == "function_definition").then_some(rhs)
    }

    /// The called name: `foo` for `foo(..)`, and the right side of `pkg::foo(..)`.
    fn callee_name(&self, call: Node) -> Option<String> {
        let function = call.child_by_field_name("function")?;
        let name = match function.kind() {
            "identifier" => function,
            "namespace_operator" => function.child_by_field_name("rhs")?,
            _ => return None,
        };
        Some(name.utf8_text(self.src).ok()?.trim_matches('`').to_string())
    }

    fn is_plain_call(&self, call: Node) -> bool {
        call.child_by_field_name("function")
            .is_some_and(|f| f.kind() == "identifier")
    }

    /// The path a `source(...)` call loads: a string literal, or `here::here("a", "b")`
    /// (-> `here:a/b`). A computed path (`source(file.path(dir, f))`) can't be known statically.
    fn source_target(&self, call: Node) -> Option<String> {
        let value = self.first_argument(call, "file")?;
        match value.kind() {
            "string" => self.string_content(value),
            "call" if self.callee_name(value).as_deref() == Some("here") => {
                let args = value.child_by_field_name("arguments")?;
                let mut parts = Vec::new();
                let mut i = 0usize;
                while i < args.child_count() {
                    if let Some(arg) = args.child(i as u32).filter(|a| a.kind() == "argument") {
                        let part = arg.child_by_field_name("value")?;
                        if part.kind() != "string" {
                            return None;
                        }
                        parts.push(self.string_content(part)?);
                    }
                    i += 1;
                }
                (!parts.is_empty()).then(|| format!("{HERE_PREFIX}{}", parts.join("/")))
            }
            _ => None,
        }
    }

    /// The value of the first positional argument, or of the one named `name`.
    fn first_argument<'t>(&self, call: Node<'t>, name: &str) -> Option<Node<'t>> {
        let args = call.child_by_field_name("arguments")?;
        let mut first_positional = None;
        let mut i = 0usize;
        while i < args.child_count() {
            if let Some(arg) = args.child(i as u32).filter(|a| a.kind() == "argument") {
                let value = arg.child_by_field_name("value");
                match arg.child_by_field_name("name") {
                    Some(n) if n.utf8_text(self.src) == Ok(name) => return value,
                    None if first_positional.is_none() => first_positional = value,
                    _ => {}
                }
            }
            i += 1;
        }
        first_positional
    }

    fn string_content(&self, string: Node) -> Option<String> {
        let content = string.child_by_field_name("content")?;
        Some(content.utf8_text(self.src).ok()?.to_string())
    }

    /// Push a symbol named by the call's first string argument (`setClass("Account", ...)`).
    fn push_string_named(&mut self, call: Node, kind: SymbolKind) {
        let Some(value) = self
            .first_argument(call, "")
            .filter(|v| v.kind() == "string")
        else {
            return;
        };
        if let (Some(name), Some(content)) = (
            self.string_content(value),
            value.child_by_field_name("content"),
        ) {
            self.symbols.push(ExtractedSymbol {
                name,
                kind,
                span: span_of(content),
            });
        }
    }

    fn push_symbol(&mut self, name_node: Node, kind: SymbolKind) {
        if let Ok(name) = name_node.utf8_text(self.src) {
            self.symbols.push(ExtractedSymbol {
                // A non-syntactic name is written in backticks: `my fun` <- function() ...
                name: name.trim_matches('`').to_string(),
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
    use compass_core::SymbolKind::{Class, Function, Method};
    use compass_extract::testing::MockResolutionContext;

    // A rich sample exercising every extraction path:
    //  - `<-`, `=` and `<<-` function assignments -> Function (incl. a backticked name and
    //    a `\(x)` lambda); a nested closure is a Function too
    //  - a plain value assignment (`threshold <- 10`) is NOT a symbol
    //  - R6Class / setClass / setRefClass -> Class named by the string; R6 `name = function`
    //    members -> Method; a non-function member (`name = NULL`) is dropped
    //  - setGeneric -> Function
    //  - imports: `library`/`require` are dropped; `source`, `sys.source`, `base::source`,
    //    `source(file = ...)` and `source(here::here(...))` survive in order; a computed
    //    path is dropped
    const SAMPLE: &str = r#"
library(dplyr)
require("stats")
source("R/utils.R")
sys.source("helpers.R", envir = env)
base::source(file = "../shared/io.R")
source(here::here("R", "model.R"))
source(file.path(dir, "dynamic.R"))

threshold <- 10

clean <- function(df, n = 1) {
  inner <- function(v) normalise(v)
  helper(df)
  dplyr::filter(df, x > n)
  df$summarise()
}

square = function(x) x^2
counter <<- function() tick()
`my fun` <- \(x) x

Person <- R6Class("Person",
  public = list(
    name = NULL,
    greet = function() {
      clean(self$name)
    }
  )
)

setClass("Account", representation(balance = "numeric"))
Stack <- setRefClass("Stack", fields = list(items = "list"))
setGeneric("deposit", function(obj, amt) standardGeneric("deposit"))

clean(data)
"#;

    fn extract(src: &str) -> Extraction {
        let x = RExtractor;
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

    fn target_of(resolved: &ResolvedImport) -> compass_core::FileId {
        match resolved {
            ResolvedImport::Resolved { target, .. } => *target,
            other => panic!("expected Resolved, got {other:?}"),
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
            ("clean".to_string(), Function),
            ("inner".to_string(), Function),   // nested closure
            ("square".to_string(), Function),  // `=` assignment
            ("counter".to_string(), Function), // `<<-` assignment
            ("my fun".to_string(), Function),  // backticks stripped, `\(x)` lambda
            ("Person".to_string(), Class),
            ("greet".to_string(), Method),
            ("Account".to_string(), Class),
            ("Stack".to_string(), Class),
            ("deposit".to_string(), Function), // S4 generic
        ];
        want.sort();

        // EXACT set: `threshold`, the `name = NULL` field, and the class-holding variables
        // (`Person <-`, `Stack <-`) must not add symbols of their own.
        assert_eq!(got, want);
    }

    #[test]
    fn extracts_source_imports_in_order() {
        let specs: Vec<String> = extract(SAMPLE)
            .imports
            .into_iter()
            .map(|i| i.specifier)
            .collect();
        assert_eq!(
            specs,
            ["R/utils.R", "helpers.R", "../shared/io.R", "here:R/model.R"]
        );
    }

    #[test]
    fn captures_plain_calls_attributed_to_the_enclosing_function() {
        let ex = extract(SAMPLE);
        let mut got: Vec<(String, String)> = ex
            .calls
            .iter()
            .map(|c| (ex.symbols[c.caller].name.clone(), c.callee.clone()))
            .collect();
        got.sort();

        // `dplyr::filter(..)` and `df$summarise()` are not plain calls; the script-level
        // `clean(data)` has no enclosing function. `function`-less constructs like
        // `standardGeneric` inside the anonymous setGeneric body have no caller symbol either.
        let want = vec![
            ("clean".to_string(), "helper".to_string()),
            ("counter".to_string(), "tick".to_string()),
            ("greet".to_string(), "clean".to_string()),
            ("inner".to_string(), "normalise".to_string()),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn resolve_tries_the_repo_root_then_the_sourcing_files_directory() {
        let ctx = MockResolutionContext::new()
            .current("analysis/run.R", "")
            .file("R/utils.R")
            .file("analysis/helpers.R")
            .file("shared/io.R");

        let imports = [
            raw("R/utils.R"),      // root-relative
            raw("helpers.R"),      // only exists next to the sourcing file
            raw("../shared/io.R"), // climbs out of analysis/
            raw("missing.R"),
        ];
        let resolved = RExtractor.resolve(&imports, &ctx, &LangConfig);

        assert_eq!(target_of(&resolved[0]), ctx.id_of("R/utils.R"));
        assert_eq!(target_of(&resolved[1]), ctx.id_of("analysis/helpers.R"));
        assert_eq!(target_of(&resolved[2]), ctx.id_of("shared/io.R"));
        assert!(matches!(resolved[3], ResolvedImport::Unresolved { .. }));
    }

    #[test]
    fn resolve_prefers_the_repo_root_when_both_locations_exist() {
        let ctx = MockResolutionContext::new()
            .current("analysis/run.R", "")
            .file("utils.R")
            .file("analysis/utils.R");

        let resolved = RExtractor.resolve(&[raw("utils.R")], &ctx, &LangConfig);
        assert_eq!(target_of(&resolved[0]), ctx.id_of("utils.R"));
    }

    #[test]
    fn resolve_here_paths_against_the_repo_root_only() {
        let ctx = MockResolutionContext::new()
            .current("analysis/run.R", "")
            .file("R/model.R")
            .file("analysis/local.R");

        let imports = [raw("here:R/model.R"), raw("here:local.R")];
        let resolved = RExtractor.resolve(&imports, &ctx, &LangConfig);

        assert_eq!(target_of(&resolved[0]), ctx.id_of("R/model.R"));
        // `here::here("local.R")` means <root>/local.R — the sibling file must not match.
        assert!(matches!(resolved[1], ResolvedImport::Unresolved { .. }));
    }

    #[test]
    fn resolve_treats_urls_and_absolute_paths_as_external() {
        let ctx = MockResolutionContext::new().current("main.R", "");
        let imports = [
            raw("https://example.com/setup.R"),
            raw("/opt/shared/setup.R"),
            raw("~/setup.R"),
            raw("C:/shared/setup.R"),
        ];
        for r in RExtractor.resolve(&imports, &ctx, &LangConfig) {
            assert!(matches!(r, ResolvedImport::External { .. }), "got {r:?}");
        }
    }

    #[test]
    fn resolve_path_collapses_segments() {
        assert_eq!(resolve_path("a/b", "util.R"), "a/b/util.R");
        assert_eq!(resolve_path("a/b", "../util.R"), "a/util.R");
        assert_eq!(resolve_path("", "./R/util.R"), "R/util.R");
        assert_eq!(resolve_path("a", "../../util.R"), "util.R");
    }
}
