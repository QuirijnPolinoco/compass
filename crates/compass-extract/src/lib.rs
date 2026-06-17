//! `compass-extract` — the STABLE contract every language plugs into.
//!
//! Defines the [`Extractor`] trait (two phases: [`Extractor::extract`] per file, then
//! [`Extractor::resolve`] over the whole repo), the supporting value types, the
//! tree-sitter parse harness, and the [`Registry`]. The language-agnostic world depends
//! on this crate; language crates implement it. See ADR-0002 and ADR-0003.

use std::path::Path;

use compass_core::{FileId, LanguageId, Span, SymbolKind};
use tree_sitter::{Language, Parser, Tree};

/// How the walker recognizes files of this language. Registry-driven: the walker holds
/// no per-language table (architecture §9).
pub struct Detection {
    pub extensions: &'static [&'static str],
    /// Substrings to look for in a `#!` first line (e.g. `"python"`).
    pub shebangs: &'static [&'static str],
}

/// A symbol emitted by an extractor, before it is interned into the graph.
pub struct ExtractedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
}

/// A raw import specifier exactly as written in source (phase 1; not yet resolved).
pub struct RawImport {
    pub specifier: String,
    pub span: Span,
}

/// Output of the per-file `extract` phase.
pub struct Extraction {
    pub symbols: Vec<ExtractedSymbol>,
    pub imports: Vec<RawImport>,
}

/// Outcome of resolving one raw import in the `resolve` phase.
pub enum ResolvedImport {
    /// Points at a real in-repo file -> becomes an `Imports` edge.
    Resolved { target: FileId, span: Span },
    /// Looked internal but matched no file -> becomes a broken-import diagnostic (FR-12/D2).
    Unresolved {
        specifier: String,
        span: Span,
        reason: String,
    },
    /// Resolved as out-of-repo (stdlib / third-party) -> no edge, no diagnostic.
    External { specifier: String },
}

/// Opaque per-language configuration carrier (e.g. derived from `.compass.toml`).
/// Reserved for future use; empty in the v1 skeleton.
#[derive(Default)]
pub struct LangConfig;

/// A read-only, language-agnostic view of the whole repo, handed to [`Extractor::resolve`].
///
/// The engine implements this; a language crate only reads from it, so no per-language
/// path logic leaks into the engine.
pub trait ResolutionContext {
    /// Absolute path to the repo root (for languages that must read project config,
    /// e.g. `go.mod`, `tsconfig.json`).
    fn repo_root(&self) -> &Path;
    /// The importing file's repo-relative path.
    fn current_file(&self) -> &Path;
    /// Resolve a repo-relative path to a `FileId`, if that file is mapped.
    fn file_by_path(&self, rel: &Path) -> Option<FileId>;
    /// All mapped files whose parent directory is exactly `rel_dir` (repo-relative).
    fn files_in_dir(&self, rel_dir: &Path) -> Vec<FileId>;
}

/// The one stable interface a language implements. The two phases keep per-language
/// resolution logic out of the engine (ADR-0002).
pub trait Extractor: Send + Sync {
    fn language_id(&self) -> LanguageId;
    fn detection(&self) -> Detection;
    /// The tree-sitter grammar for this language.
    fn grammar(&self) -> Language;
    /// Phase 1 (per file): pull symbols + raw import specifiers from a parsed tree.
    fn extract(&self, source: &[u8], tree: &Tree) -> Extraction;
    /// Phase 2 (whole repo): resolve raw imports to files using `ctx`. The algorithm is
    /// language-specific; the engine only supplies the context.
    fn resolve(
        &self,
        imports: &[RawImport],
        ctx: &dyn ResolutionContext,
        config: &LangConfig,
    ) -> Vec<ResolvedImport>;
}

/// Parse `source` with `grammar` into a tree-sitter [`Tree`] — the shared harness so no
/// language crate sets up a parser itself.
pub fn parse(grammar: &Language, source: &[u8]) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(grammar).ok()?;
    parser.parse(source, None)
}

/// The set of compiled-in extractors. Populated by the CLI composition root via the
/// explicit `register_all()` (ADR-0003) — no linker-section magic.
#[derive(Default)]
pub struct Registry {
    extractors: Vec<Box<dyn Extractor>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, extractor: Box<dyn Extractor>) {
        self.extractors.push(extractor);
    }

    pub fn extractors(&self) -> &[Box<dyn Extractor>] {
        &self.extractors
    }

    /// Single source of truth for FR-14/H2: the languages actually compiled in.
    pub fn language_ids(&self) -> Vec<LanguageId> {
        self.extractors.iter().map(|e| e.language_id()).collect()
    }

    /// Pick the extractor for a file by extension first, then by shebang first line.
    pub fn detect(&self, path: &Path, first_line: Option<&str>) -> Option<&dyn Extractor> {
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            for e in &self.extractors {
                if e.detection().extensions.contains(&ext) {
                    return Some(e.as_ref());
                }
            }
        }
        if let Some(line) = first_line {
            if line.starts_with("#!") {
                for e in &self.extractors {
                    if e.detection().shebangs.iter().any(|s| line.contains(s)) {
                        return Some(e.as_ref());
                    }
                }
            }
        }
        None
    }
}
