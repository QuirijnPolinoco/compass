//! `compass-core` — the language-agnostic domain model for Compass.
//!
//! Holds the graph (files, symbols, edges), the diagnostics sink, and the read-only
//! query port the MCP layer talks to. It knows nothing about MCP, tree-sitter, or any
//! specific language. See `docs/architecture/02-architecture.md` §4–§5.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Open identifier for a language (e.g. `"go"`).
///
/// Deliberately NOT an enum: languages are plugins, so adding one must never edit core
/// (the North Star, ADR-0002).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LanguageId(String);

impl LanguageId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LanguageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FileId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SymbolId(pub u32);

/// A mapped source file (graph node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    pub id: FileId,
    /// Repo-relative path, forward-slash normalized for stable cross-platform output.
    pub path: PathBuf,
    pub language: LanguageId,
    /// Hash of the file contents — drives incremental staleness detection.
    pub content_hash: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Interface,
    Enum,
    Constant,
    Variable,
    Module,
    Other,
}

/// A source location (byte range + start row/col), enough to jump to a symbol.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Span {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_row: usize,
    pub start_col: usize,
}

/// A defined symbol (graph node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub file: FileId,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticKind {
    /// A file (or part of it) could not be parsed; contained, never fatal.
    ParseError,
    /// An import looked internal but resolved to no real file (FR-12/D2).
    UnresolvedImport,
}

/// A non-fatal issue. The universal sink: collected, never crashes the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub file: FileId,
    pub message: String,
}

/// The map: nodes + edges + diagnostics.
///
/// Single-writer, in-memory. Its serde form is a versioned cache surface (ADR-0004);
/// transient indices (`by_path`) are rebuilt via [`Graph::reindex`] after load.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Graph {
    files: Vec<File>,
    symbols: Vec<Symbol>,
    imports: Vec<(FileId, FileId)>,
    defines: Vec<(FileId, SymbolId)>,
    calls: Vec<(SymbolId, SymbolId)>,
    diagnostics: Vec<Diagnostic>,
    #[serde(skip)]
    by_path: HashMap<PathBuf, FileId>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(&mut self, path: PathBuf, language: LanguageId, content_hash: u64) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.by_path.insert(path.clone(), id);
        self.files.push(File {
            id,
            path,
            language,
            content_hash,
        });
        id
    }

    pub fn add_symbol(
        &mut self,
        name: String,
        kind: SymbolKind,
        file: FileId,
        span: Span,
    ) -> SymbolId {
        let id = SymbolId(self.symbols.len() as u32);
        self.symbols.push(Symbol {
            id,
            name,
            kind,
            file,
            span,
        });
        self.defines.push((file, id));
        id
    }

    pub fn add_import(&mut self, from: FileId, to: FileId) {
        self.imports.push((from, to));
    }

    pub fn add_call(&mut self, from: SymbolId, to: SymbolId) {
        self.calls.push((from, to));
    }

    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn files(&self) -> &[File] {
        &self.files
    }

    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    pub fn imports(&self) -> &[(FileId, FileId)] {
        &self.imports
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Resolve a repo-relative path to its `FileId`, if mapped.
    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.by_path.get(path).copied()
    }

    /// The repo-relative path of a file node.
    pub fn file_path(&self, id: FileId) -> Option<&Path> {
        self.files.get(id.0 as usize).map(|f| f.path.as_path())
    }

    /// Rebuild transient indices after deserializing from cache.
    pub fn reindex(&mut self) {
        self.by_path = self.files.iter().map(|f| (f.path.clone(), f.id)).collect();
    }
}

/// Read-only query port the MCP layer depends on, so it never touches the engine
/// (ADR-0002 / architecture §4). The concrete graph implements it.
pub trait MapQuery {
    fn overview(&self) -> Overview;
    /// What `file` imports, and what imports it (FR-10/B2). `file` is a repo-relative,
    /// forward-slash path. Returns `None` if the file isn't in the map.
    fn file_dependencies(&self, file: &str) -> Option<FileDependencies>;
    /// Imports that point at no real file — mistakes to catch early (FR-12/D2).
    fn broken_imports(&self) -> Vec<BrokenImport>;
}

/// A high-level summary of the map (FR-3/B1, the `overview` MCP tool).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Overview {
    pub file_count: usize,
    pub symbol_count: usize,
    pub import_edge_count: usize,
    pub diagnostic_count: usize,
    pub languages: Vec<LanguageStat>,
    /// The files with the most import connections — where the important logic tends to
    /// live (FR-16/B3). Capped to the top handful.
    pub most_connected: Vec<ConnectedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageStat {
    pub language: LanguageId,
    pub file_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectedFile {
    pub file: String,
    /// Number of import edges touching this file (in + out).
    pub connections: usize,
}

/// What a file depends on and what depends on it (FR-10/B2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDependencies {
    pub file: String,
    /// Files this file imports.
    pub dependencies: Vec<String>,
    /// Files that import this file.
    pub dependents: Vec<String>,
}

/// An import that resolved to no real file (FR-12/D2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokenImport {
    pub file: String,
    pub message: String,
}

impl MapQuery for Graph {
    fn overview(&self) -> Overview {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for f in &self.files {
            *counts.entry(f.language.as_str()).or_insert(0) += 1;
        }
        let mut languages: Vec<LanguageStat> = counts
            .into_iter()
            .map(|(l, c)| LanguageStat {
                language: LanguageId::new(l),
                file_count: c,
            })
            .collect();
        languages.sort_by(|a, b| {
            b.file_count
                .cmp(&a.file_count)
                .then_with(|| a.language.as_str().cmp(b.language.as_str()))
        });
        let mut degree: HashMap<FileId, usize> = HashMap::new();
        for (from, to) in &self.imports {
            *degree.entry(*from).or_insert(0) += 1;
            *degree.entry(*to).or_insert(0) += 1;
        }
        let mut most_connected: Vec<ConnectedFile> = degree
            .into_iter()
            .filter_map(|(id, connections)| {
                self.file_path(id).map(|p| ConnectedFile {
                    file: p.to_string_lossy().into_owned(),
                    connections,
                })
            })
            .collect();
        most_connected.sort_by(|a, b| {
            b.connections
                .cmp(&a.connections)
                .then_with(|| a.file.cmp(&b.file))
        });
        most_connected.truncate(10);

        Overview {
            file_count: self.files.len(),
            symbol_count: self.symbols.len(),
            import_edge_count: self.imports.len(),
            diagnostic_count: self.diagnostics.len(),
            languages,
            most_connected,
        }
    }

    fn file_dependencies(&self, file: &str) -> Option<FileDependencies> {
        let fid = self.file_id(Path::new(file))?;

        let mut dependencies: Vec<String> = self
            .imports
            .iter()
            .filter(|(from, _)| *from == fid)
            .filter_map(|(_, to)| self.file_path(*to))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        dependencies.sort();
        dependencies.dedup();

        let mut dependents: Vec<String> = self
            .imports
            .iter()
            .filter(|(_, to)| *to == fid)
            .filter_map(|(from, _)| self.file_path(*from))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        dependents.sort();
        dependents.dedup();

        Some(FileDependencies {
            file: file.to_string(),
            dependencies,
            dependents,
        })
    }

    fn broken_imports(&self) -> Vec<BrokenImport> {
        self.diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UnresolvedImport)
            .filter_map(|d| {
                let file = self.file_path(d.file)?.to_string_lossy().into_owned();
                Some(BrokenImport {
                    file,
                    message: d.message.clone(),
                })
            })
            .collect()
    }
}
