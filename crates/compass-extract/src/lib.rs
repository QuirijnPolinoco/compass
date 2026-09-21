//! `compass-extract` — the STABLE contract every language plugs into.
//!
//! Defines the [`Extractor`] trait (two phases: [`Extractor::extract`] per file, then
//! [`Extractor::resolve`] over the whole repo), the supporting value types, the
//! tree-sitter parse harness, and the [`Registry`]. The language-agnostic world depends
//! on this crate; language crates implement it. See ADR-0002 and ADR-0003.

use std::path::Path;

use compass_core::{EdgeConfidence, FileCategory, FileId, LanguageId, Span, SymbolKind};
use serde::{Deserialize, Serialize};
use tree_sitter::{Language, Parser, Tree};

/// How the walker recognizes files of this language. Registry-driven: the walker holds
/// no per-language table (architecture §9).
pub struct Detection {
    pub extensions: &'static [&'static str],
    /// Substrings to look for in a `#!` first line (e.g. `"python"`).
    pub shebangs: &'static [&'static str],
}

/// A symbol emitted by an extractor, before it is interned into the graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub span: Span,
}

/// A raw import specifier exactly as written in source (phase 1; not yet resolved).
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Points at a real in-repo file -> becomes an `Imports` edge. `confidence` records how the
    /// resolver found the target: [`EdgeConfidence::Resolved`] for a path-exact hit (a file that
    /// provably exists), [`EdgeConfidence::Heuristic`] for a convention-based guess (e.g. a
    /// namespace mapped to a directory). Use the [`resolved`](Self::resolved) /
    /// [`heuristic`](Self::heuristic) constructors rather than building this directly.
    Resolved {
        target: FileId,
        span: Span,
        confidence: EdgeConfidence,
    },
    /// Looked internal but matched no file -> becomes a broken-import diagnostic (FR-12/D2).
    Unresolved {
        specifier: String,
        span: Span,
        reason: String,
    },
    /// Resolved as out-of-repo (stdlib / third-party) -> no edge, no diagnostic.
    External { specifier: String },
}

impl ResolvedImport {
    /// A path-exact import: the target file provably exists (relative path, `mod`/`include`,
    /// tsconfig alias, …). Tagged [`EdgeConfidence::Resolved`].
    pub fn resolved(target: FileId, span: Span) -> Self {
        ResolvedImport::Resolved {
            target,
            span,
            confidence: EdgeConfidence::Resolved,
        }
    }

    /// A convention-based import: the target is a best-effort guess (e.g. namespace→directory).
    /// Tagged [`EdgeConfidence::Heuristic`].
    pub fn heuristic(target: FileId, span: Span) -> Self {
        ResolvedImport::Resolved {
            target,
            span,
            confidence: EdgeConfidence::Heuristic,
        }
    }
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
    /// Every mapped file's repo-relative path (order unspecified). Lets a resolver discover
    /// repo-wide structure — e.g. *all* source roots in a multi-module project — instead of
    /// reasoning only from the importing file. Resolvers should cache anything derived from
    /// this (it's the same for every file in a run); see the Rust/Java extractors.
    fn all_files(&self) -> Vec<&Path>;
}

/// How an extractor gets at a file's contents.
pub enum Parsing {
    /// Parse the whole file with a tree-sitter grammar, then [`Extractor::extract`]. What every
    /// programming language does.
    Grammar(Language),
    /// Read at most `max_bytes` from the top of the file and hand them to
    /// [`Extractor::extract_head`] — for file types whose *schema* is at the top and whose body
    /// may be gigabytes (a CSV header). Such a file is never parsed, so the engine's size cap
    /// does not apply to it: a 2 GB export costs the same as a 2 KB one.
    Head { max_bytes: usize },
}

/// The one stable interface a language implements. The two phases keep per-language
/// resolution logic out of the engine (ADR-0002).
pub trait Extractor: Send + Sync {
    fn language_id(&self) -> LanguageId;
    fn detection(&self) -> Detection;
    /// Exact file names this extractor claims (`package.json`, `Dockerfile`), for file types that
    /// are identified by *name* rather than extension (ADR-0007 §3). A file-name claim beats any
    /// extension claim, whichever extractor makes it, so a manifest extractor can own
    /// `package.json` without anyone claiming `.json`. Exact names only — no globs — so what a
    /// build maps stays predictable. Defaults to none.
    fn filenames(&self) -> &'static [&'static str] {
        &[]
    }
    /// What kind of file this extractor maps (ADR-0007). Defaults to source code. Anything that
    /// is not [code-like](FileCategory::is_code_like) is a *supporting* file: it is in the map
    /// and searchable, but never counts as a dependent, a hub or a cycle member.
    fn category(&self) -> FileCategory {
        FileCategory::code()
    }
    /// The namespace this language's calls resolve in. A call only ever links to a symbol
    /// from the same namespace, so a Python `build()` can't be matched to a Go `build` — and,
    /// just as important, a name that is unique *within* a language stays resolvable no matter
    /// what other languages define. Defaults to the language itself; languages that really do
    /// call into each other (C and C++, Java and Kotlin) return a shared name.
    fn call_namespace(&self) -> String {
        self.language_id().as_str().to_string()
    }
    /// The tree-sitter grammar for this language. Every programming language implements this;
    /// only an extractor that overrides [`parsing`](Self::parsing) may leave it out.
    fn grammar(&self) -> Language {
        unimplemented!(
            "{}: implement `grammar()`, or override `parsing()` to read files another way",
            self.language_id()
        )
    }
    /// How this extractor reads a file. Defaults to parsing it with [`grammar`](Self::grammar);
    /// the engine only ever asks this, never `grammar()` directly.
    fn parsing(&self) -> Parsing {
        Parsing::Grammar(self.grammar())
    }
    /// Phase 1 (per file): pull symbols + raw import specifiers from a parsed tree. Like
    /// [`grammar`](Self::grammar), required of every extractor that parses — which is why the
    /// default fails loudly instead of quietly mapping nothing.
    fn extract(&self, _source: &[u8], _tree: &Tree) -> Extraction {
        unimplemented!(
            "{}: implement `extract()`, or override `parsing()` and `extract_head()`",
            self.language_id()
        )
    }
    /// Phase 1 for [`Parsing::Head`] extractors: pull symbols from the top of a file. `head` may
    /// end mid-line or mid-character — it is a prefix, not the file.
    fn extract_head(&self, _head: &[u8]) -> Extraction {
        Extraction::default()
    }
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

    /// The registered extractor for `language`, if this build has one.
    pub fn extractor_for(&self, language: &LanguageId) -> Option<&dyn Extractor> {
        self.extractors
            .iter()
            .find(|e| &e.language_id() == language)
            .map(|e| e.as_ref())
    }

    /// Whether `path` can only be identified by its `#!` line: no extractor claims its name or
    /// extension, and it has no extension at all (`bin/deploy`, `scripts/migrate`). These are
    /// the only files whose first line is worth reading.
    pub fn needs_first_line(&self, path: &Path) -> bool {
        path.extension().is_none() && self.detect(path, None).is_none()
    }

    /// Pick the extractor for a file: by exact file name first (across *all* extractors, so a
    /// name claim beats an extension claim), then by extension, then by shebang first line.
    pub fn detect(&self, path: &Path, first_line: Option<&str>) -> Option<&dyn Extractor> {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            for e in &self.extractors {
                if e.filenames().contains(&name) {
                    return Some(e.as_ref());
                }
            }
        }
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
        /// resolvers that re-read it (for a package/namespace line) work. The importing file is
        /// itself a mapped file in a real index, so it's registered too (unless already added)
        /// — this is what puts it in [`all_files`](ResolutionContext::all_files).
        pub fn current(mut self, rel: &str, contents: &str) -> Self {
            self.write(rel, contents);
            if !self.by_path.contains_key(Path::new(rel)) {
                self.register(rel);
            }
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
        fn all_files(&self) -> Vec<&Path> {
            self.by_path.keys().map(PathBuf::as_path).collect()
        }
    }

    fn parent_dir(rel: &Path) -> PathBuf {
        match rel.to_string_lossy().rsplit_once('/') {
            Some((dir, _)) => PathBuf::from(dir),
            None => PathBuf::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A detection-only test double: `detect` never parses, so the rest is unreachable.
    struct Fake {
        id: &'static str,
        extensions: &'static [&'static str],
        shebangs: &'static [&'static str],
        filenames: &'static [&'static str],
    }

    impl Extractor for Fake {
        fn language_id(&self) -> LanguageId {
            LanguageId::new(self.id)
        }
        fn detection(&self) -> Detection {
            Detection {
                extensions: self.extensions,
                shebangs: self.shebangs,
            }
        }
        fn filenames(&self) -> &'static [&'static str] {
            self.filenames
        }
        fn grammar(&self) -> Language {
            unreachable!("detection never parses")
        }
        fn extract(&self, _: &[u8], _: &Tree) -> Extraction {
            unreachable!("detection never parses")
        }
        fn resolve(
            &self,
            _: &[RawImport],
            _: &dyn ResolutionContext,
            _: &LangConfig,
        ) -> Vec<ResolvedImport> {
            unreachable!("detection never parses")
        }
    }

    fn registry() -> Registry {
        let mut registry = Registry::new();
        // Registered FIRST and claims the extension: a later file-name claim must still win.
        registry.register(Box::new(Fake {
            id: "json",
            extensions: &["json"],
            shebangs: &[],
            filenames: &[],
        }));
        registry.register(Box::new(Fake {
            id: "npm",
            extensions: &[],
            shebangs: &[],
            filenames: &["package.json"],
        }));
        registry.register(Box::new(Fake {
            id: "shell",
            extensions: &["sh"],
            shebangs: &["bash", "/sh"],
            filenames: &[],
        }));
        registry
    }

    fn detected(registry: &Registry, path: &str, first_line: Option<&str>) -> Option<String> {
        registry
            .detect(Path::new(path), first_line)
            .map(|e| e.language_id().to_string())
    }

    #[test]
    fn a_file_name_claim_beats_an_extension_claim() {
        let r = registry();
        assert_eq!(
            detected(&r, "web/package.json", None).as_deref(),
            Some("npm")
        );
        assert_eq!(
            detected(&r, "data/users.json", None).as_deref(),
            Some("json")
        );
        // Exact names only: no prefix, suffix or case games.
        assert_eq!(detected(&r, "package.json.bak", None), None);
        assert_eq!(
            detected(&r, "my-package.json", None).as_deref(),
            Some("json")
        );
    }

    #[test]
    fn a_shebang_identifies_an_extensionless_script() {
        let r = registry();
        assert_eq!(
            detected(&r, "bin/deploy", Some("#!/usr/bin/env bash")).as_deref(),
            Some("shell")
        );
        assert_eq!(
            detected(&r, "bin/deploy", Some("#!/bin/sh")).as_deref(),
            Some("shell")
        );
        assert_eq!(detected(&r, "bin/deploy", Some("echo bash")), None); // not a #! line
        assert_eq!(detected(&r, "LICENSE", None), None);
    }

    #[test]
    fn only_unclaimed_extensionless_files_need_their_first_line_read() {
        let r = registry();
        assert!(r.needs_first_line(Path::new("bin/deploy")));
        assert!(!r.needs_first_line(Path::new("run.sh"))); // extension claimed
        assert!(!r.needs_first_line(Path::new("notes.txt"))); // has an extension: never a script
        assert!(!r.needs_first_line(Path::new("package.json"))); // name claimed
    }

    #[test]
    fn extractor_for_finds_a_registered_language() {
        let r = registry();
        assert!(r.extractor_for(&LanguageId::new("npm")).is_some());
        assert!(r.extractor_for(&LanguageId::new("cobol")).is_none());
    }
}
