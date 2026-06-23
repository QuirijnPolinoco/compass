//! Full-index orchestration: walk → (parallel) parse+extract → assemble → resolve.
//! See architecture §6 Flow A. The two-phase extractor keeps per-language resolution
//! logic out of this engine (ADR-0002).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use compass_core::{Diagnostic, DiagnosticKind, FileId, Graph, LanguageId, SymbolId};
use compass_extract::{
    ExtractedSymbol, LangConfig, RawCall, RawImport, Registry, ResolutionContext, ResolvedImport,
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
    calls: Vec<RawCall>,
}

/// Build the full map of `repo_root` using the compiled-in language extractors.
pub fn index(repo_root: &Path, registry: &Registry) -> anyhow::Result<Graph> {
    let files = walk::walk(repo_root);

    // PHASE 1 — parallel parse + extract. Each file is independent (rayon, no GIL).
    let parsed: Vec<Parsed> = files
        .par_iter()
        .filter_map(|w| parse_one(w, registry))
        .collect();

    // Assemble nodes (single-writer). Keep each file's symbol ids in extraction order so a
    // `RawCall.caller`/callee index can be mapped back to a real `SymbolId` below.
    let mut graph = Graph::new();
    let mut symbol_ids: Vec<Vec<SymbolId>> = Vec::with_capacity(parsed.len());
    for p in &parsed {
        let fid = graph.add_file(p.rel.clone(), p.language.clone(), p.hash);
        let mut ids = Vec::with_capacity(p.symbols.len());
        for s in &p.symbols {
            ids.push(graph.add_symbol(s.name.clone(), s.kind, fid, s.span));
        }
        symbol_ids.push(ids);
    }

    // Resolve calls into symbol→symbol edges (ADR-0002 keeps this language-agnostic: extractors
    // only emit raw caller/callee names). A `Calls` edge is added only when the callee is
    // unambiguous — the same-file symbol of that name, else a *unique* global match. Ambiguous
    // names (overloads, common method names) are skipped so we never draw a wrong edge.
    resolve_calls(&mut graph, &parsed, &symbol_ids);

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
    let tree = compass_extract::parse(&grammar, &bytes)?;
    let extraction = extractor.extract(&bytes, &tree);
    Some(Parsed {
        rel: w.rel.clone(),
        language: extractor.language_id(),
        hash: content_hash(&bytes),
        symbols: extraction.symbols,
        imports: extraction.imports,
        calls: extraction.calls,
    })
}

/// Turn raw caller/callee names into `Calls` edges. Conservative by design: a call resolves to
/// the same-file symbol of that name first, otherwise to a *unique* global match — names that
/// occur in more than one file (and aren't local) are left unresolved rather than guessed.
fn resolve_calls(graph: &mut Graph, parsed: &[Parsed], symbol_ids: &[Vec<SymbolId>]) {
    // Global name → symbol ids (across the whole repo).
    let mut by_name: HashMap<&str, Vec<SymbolId>> = HashMap::new();
    for (pi, p) in parsed.iter().enumerate() {
        for (si, s) in p.symbols.iter().enumerate() {
            by_name
                .entry(s.name.as_str())
                .or_default()
                .push(symbol_ids[pi][si]);
        }
    }

    for (pi, p) in parsed.iter().enumerate() {
        if p.calls.is_empty() {
            continue;
        }
        let ids = &symbol_ids[pi];
        // Same-file name → symbol id (first definition wins).
        let mut local: HashMap<&str, SymbolId> = HashMap::new();
        for (si, s) in p.symbols.iter().enumerate() {
            local.entry(s.name.as_str()).or_insert(ids[si]);
        }
        for call in &p.calls {
            let Some(&caller) = ids.get(call.caller) else {
                continue;
            };
            let target = local.get(call.callee.as_str()).copied().or_else(|| {
                match by_name.get(call.callee.as_str()) {
                    Some(matches) if matches.len() == 1 => Some(matches[0]),
                    _ => None,
                }
            });
            if let Some(callee) = target {
                if callee != caller {
                    graph.add_call(caller, callee);
                }
            }
        }
    }
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
