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
#[derive(Debug, Clone)]
pub struct ExtractedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
}

/// A raw import specifier exactly as written in source (phase 1; not yet resolved).
#[derive(Debug, Clone)]
pub struct RawImport {
    pub specifier: String,
    pub span: Span,
}

/// A call discovered in a file (phase 1), before it is resolved to a symbol.
///
/// `caller` indexes into the same [`Extraction`]'s `symbols` (the enclosing function/method);
/// `callee` is the called name. The engine resolves `callee` to a `SymbolId` — same-file
/// first, then a unique global match — and skips ambiguous names, so a `Calls` edge is only
/// ever added when the target is unambiguous. Extractors that don't track calls leave this
/// empty (it defaults to `[]`).
#[derive(Debug, Clone)]
pub struct RawCall {
    pub caller: usize,
    pub callee: String,
    pub span: Span,
}

/// Output of the per-file `extract` phase.
#[derive(Debug, Default)]
pub struct Extraction {
    pub symbols: Vec<ExtractedSymbol>,
    pub imports: Vec<RawImport>,
    pub calls: Vec<RawCall>,
}

/// Outcome of resolving one raw import in the `resolve` phase.
#[derive(Debug, Clone)]
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

/// Test support: a [`ResolutionContext`] for unit-testing language `resolve()` phases.
/// Enabled by the `test-util` feature (lang crates turn it on as a dev-dependency).
#[cfg(feature = "test-util")]
pub mod testing {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use compass_core::FileId;

    use crate::ResolutionContext;

    /// A [`ResolutionContext`] backed by a real temp dir, so resolvers that read project
    /// config from disk (`go.mod`, a file's `package`/`namespace` line) work too.
    ///
    /// Build it fluently: [`file`](Self::file) registers a mapped file, [`disk`](Self::disk)
    /// writes on-disk config, [`current`](Self::current) sets the importing file. Use
    /// [`id_of`](Self::id_of) to assert which file an import resolved to.
    pub struct MockResolutionContext {
        tmp: tempfile::TempDir,
        current_file: PathBuf,
        by_path: HashMap<PathBuf, FileId>,
        by_dir: HashMap<PathBuf, Vec<FileId>>,
        next_id: u32,
    }

    impl Default for MockResolutionContext {
        fn default() -> Self {
            Self {
                tmp: tempfile::tempdir().expect("create temp dir"),
                current_file: PathBuf::new(),
                by_path: HashMap::new(),
                by_dir: HashMap::new(),
                next_id: 0,
            }
        }
    }

    impl MockResolutionContext {
        pub fn new() -> Self {
            Self::default()
        }

        /// Register a mapped file at `rel` (repo-relative, `/`-separated), empty on disk.
        pub fn file(mut self, rel: &str) -> Self {
            self.write(rel, "");
            self.register(rel);
            self
        }

        /// Register a mapped file at `rel` with on-disk `contents` (e.g. `go.mod`).
        pub fn disk(mut self, rel: &str, contents: &str) -> Self {
            self.write(rel, contents);
            self.register(rel);
            self
        }

        /// Set the importing file (repo-relative), written to disk with `contents` so
        /// resolvers that re-read it (for a package/namespace line) work.
        pub fn current(mut self, rel: &str, contents: &str) -> Self {
            self.write(rel, contents);
            self.current_file = PathBuf::from(rel);
            self
        }

        /// The `FileId` assigned to a registered path — for asserting resolution targets.
        pub fn id_of(&self, rel: &str) -> FileId {
            self.by_path[&PathBuf::from(rel)]
        }

        fn register(&mut self, rel: &str) -> FileId {
            let id = FileId(self.next_id);
            self.next_id += 1;
            let path = PathBuf::from(rel);
            self.by_dir.entry(parent_dir(&path)).or_default().push(id);
            self.by_path.insert(path, id);
            id
        }

        fn write(&self, rel: &str, contents: &str) {
            let abs = self.tmp.path().join(rel);
            if let Some(parent) = abs.parent() {
                std::fs::create_dir_all(parent).expect("create dirs");
            }
            std::fs::write(&abs, contents).expect("write file");
        }
    }

    impl ResolutionContext for MockResolutionContext {
        fn repo_root(&self) -> &Path {
            self.tmp.path()
        }
        fn current_file(&self) -> &Path {
            &self.current_file
        }
        fn file_by_path(&self, rel: &Path) -> Option<FileId> {
            self.by_path.get(rel).copied()
        }
        fn files_in_dir(&self, rel_dir: &Path) -> Vec<FileId> {
            self.by_dir.get(rel_dir).cloned().unwrap_or_default()
        }
    }

    fn parent_dir(rel: &Path) -> PathBuf {
        match rel.to_string_lossy().rsplit_once('/') {
            Some((dir, _)) => PathBuf::from(dir),
            None => PathBuf::new(),
        }
    }
}
