//! Full-index orchestration: walk → (parallel) parse+extract → assemble → resolve.
//! See architecture §6 Flow A. The two-phase extractor keeps per-language resolution
//! logic out of this engine (ADR-0002).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mapai_core::{Diagnostic, DiagnosticKind, FileId, Graph, LanguageId};
use mapai_extract::{
    ExtractedSymbol, LangConfig, RawImport, Registry, ResolutionContext, ResolvedImport,
};
use rayon::prelude::*;

use crate::walk::{self, Walked};

/// One file after phase-1 parse + extract.
struct Parsed {
    rel: PathBuf,
    language: LanguageId,
    hash: u64,
    symbols: Vec<ExtractedSymbol>,
    imports: Vec<RawImport>,
}

/// Build the full map of `repo_root` using the compiled-in language extractors.
pub fn index(repo_root: &Path, registry: &Registry) -> anyhow::Result<Graph> {
    let files = walk::walk(repo_root);

    // PHASE 1 — parallel parse + extract. Each file is independent (rayon, no GIL).
    let parsed: Vec<Parsed> = files
        .par_iter()
        .filter_map(|w| parse_one(w, registry))
        .collect();

    // Assemble nodes (single-writer).
    let mut graph = Graph::new();
    for p in &parsed {
        let fid = graph.add_file(p.rel.clone(), p.language.clone(), p.hash);
        for s in &p.symbols {
            graph.add_symbol(s.name.clone(), s.kind, fid, s.span);
        }
    }

    // Build the language-agnostic resolution indices from the assembled files.
    let mut by_path: HashMap<PathBuf, FileId> = HashMap::new();
    let mut by_dir: HashMap<PathBuf, Vec<FileId>> = HashMap::new();
    for f in graph.files() {
        by_path.insert(f.path.clone(), f.id);
        by_dir
            .entry(walk::parent_dir(&f.path))
            .or_default()
            .push(f.id);
    }

    // PHASE 2 — resolve imports via each language's own algorithm (through the trait).
    let config = LangConfig;
    for p in &parsed {
        let Some(extractor) = registry.detect(&p.rel, None) else {
            continue;
        };
        let fid = by_path[&p.rel];
        let ctx = RepoContext {
            repo_root,
            current_file: p.rel.clone(),
            by_path: &by_path,
            by_dir: &by_dir,
        };
        for resolved in extractor.resolve(&p.imports, &ctx, &config) {
            match resolved {
                ResolvedImport::Resolved { target, .. } => {
                    if target != fid {
                        graph.add_import(fid, target);
                    }
                }
                ResolvedImport::Unresolved {
                    specifier, reason, ..
                } => graph.add_diagnostic(Diagnostic {
                    kind: DiagnosticKind::UnresolvedImport,
                    file: fid,
                    message: format!("unresolved import `{specifier}`: {reason}"),
                }),
                ResolvedImport::External { .. } => {}
            }
        }
    }

    Ok(graph)
}

fn parse_one(w: &Walked, registry: &Registry) -> Option<Parsed> {
    let extractor = registry.detect(&w.rel, None)?;
    let bytes = std::fs::read(&w.abs).ok()?;
    let grammar = extractor.grammar();
    let tree = mapai_extract::parse(&grammar, &bytes)?;
    let extraction = extractor.extract(&bytes, &tree);
    Some(Parsed {
        rel: w.rel.clone(),
        language: extractor.language_id(),
        hash: content_hash(&bytes),
        symbols: extraction.symbols,
        imports: extraction.imports,
    })
}

fn content_hash(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// The engine's concrete [`ResolutionContext`]: a read-only, language-agnostic view of
/// the repo handed to each extractor's `resolve` phase.
struct RepoContext<'a> {
    repo_root: &'a Path,
    current_file: PathBuf,
    by_path: &'a HashMap<PathBuf, FileId>,
    by_dir: &'a HashMap<PathBuf, Vec<FileId>>,
}

impl ResolutionContext for RepoContext<'_> {
    fn repo_root(&self) -> &Path {
        self.repo_root
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
