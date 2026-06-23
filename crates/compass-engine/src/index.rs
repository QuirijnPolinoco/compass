//! Full-index orchestration: walk → (parallel) parse+extract → assemble → resolve.
//! See architecture §6 Flow A. The two-phase extractor keeps per-language resolution
//! logic out of this engine (ADR-0002).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use compass_core::{
    Diagnostic, DiagnosticKind, EdgeConfidence, FileId, Graph, LanguageId, SymbolId,
};
use compass_extract::{
    ExtractedSymbol, LangConfig, RawCall, RawImport, Registry, ResolutionContext, ResolvedImport,
};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::walk::{self, Walked};

/// One file after phase-1 parse + extract. Carries its change fingerprint (`mtime_ns`/`size`)
/// so the result can be written back into the [`ExtractionCache`] for next time.
struct Parsed {
    rel: PathBuf,
    language: LanguageId,
    hash: u64,
    symbols: Vec<ExtractedSymbol>,
    imports: Vec<RawImport>,
    calls: Vec<RawCall>,
    mtime_ns: u64,
    size: u64,
}

/// A file's phase-1 extraction, cached on disk so a later index can skip re-reading/parsing it
/// when `(mtime_ns, size)` are unchanged — on a large repo the dominant cost is reading every
/// file, not parsing it. Keyed by repo-relative path in the [`ExtractionCache`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedFile {
    pub mtime_ns: u64,
    pub size: u64,
    pub language: LanguageId,
    pub hash: u64,
    pub symbols: Vec<ExtractedSymbol>,
    pub imports: Vec<RawImport>,
    pub calls: Vec<RawCall>,
}

/// Repo-relative path → its last phase-1 extraction. Persisted by the `cache` module and fed
/// back into [`index_incremental`] to avoid redundant reads/parses.
pub type ExtractionCache = HashMap<String, CachedFile>;

/// Build the full map of `repo_root`, reading and parsing every file. A convenience wrapper over
/// [`index_incremental`] for callers that don't keep an extraction cache (e.g. tests).
pub fn index(repo_root: &Path, registry: &Registry) -> anyhow::Result<Graph> {
    Ok(index_incremental(repo_root, registry, None)?.0)
}

/// Like [`index`], but reuse phase-1 results from `prev` for files whose `(mtime_ns, size)`
/// match, re-reading/parsing only changed or new files — the big win on large repos, where the
/// cost is reading every file, not parsing it. The graph is still rebuilt in full from the
/// *complete current* file set (assemble + resolve + calls are cheap), so adds/deletes/renames
/// stay correct — only the expensive per-file read+parse is skipped. Returns the graph plus a
/// fresh [`ExtractionCache`] to persist for next time.
///
/// Set `COMPASS_TIMING=1` to print per-phase wall-clock to stderr — the cheap way to see where
/// indexing a large repo spends its time (walk vs parse vs resolve).
pub fn index_incremental(
    repo_root: &Path,
    registry: &Registry,
    prev: Option<&ExtractionCache>,
) -> anyhow::Result<(Graph, ExtractionCache)> {
    let walk_t = PhaseTimer::start("walk");
    let files = walk::walk(repo_root);
    walk_t.stop(files.len());

    // PHASE 1 — parallel parse + extract, reusing an unchanged file's cached extraction instead
    // of re-reading it. Each file is independent (rayon, no GIL).
    let parse_t = PhaseTimer::start("parse+extract");
    let parsed: Vec<Parsed> = files
        .par_iter()
        .filter_map(|w| reuse_or_parse(w, registry, prev))
        .collect();
    parse_t.stop(parsed.len());

    // Assemble nodes (single-writer). Keep each file's symbol ids in extraction order so a
    // `RawCall.caller`/callee index can be mapped back to a real `SymbolId` below.
    let assemble_t = PhaseTimer::start("assemble");
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
    assemble_t.stop(graph.symbols().len());

    // Resolve calls into symbol→symbol edges (ADR-0002 keeps this language-agnostic: extractors
    // only emit raw caller/callee names). A `Calls` edge is added only when the callee is
    // unambiguous — the same-file symbol of that name, else a *unique* global match. Ambiguous
    // names (overloads, common method names) are skipped so we never draw a wrong edge.
    let calls_t = PhaseTimer::start("resolve-calls");
    resolve_calls(&mut graph, &parsed, &symbol_ids);
    calls_t.stop(graph.calls().len());

    // Build the language-agnostic resolution indices from the assembled files.
    let index_t = PhaseTimer::start("build-indices");
    let mut by_path: HashMap<PathBuf, FileId> = HashMap::new();
    let mut by_dir: HashMap<PathBuf, Vec<FileId>> = HashMap::new();
    for f in graph.files() {
        by_path.insert(f.path.clone(), f.id);
        by_dir
            .entry(walk::parent_dir(&f.path))
            .or_default()
            .push(f.id);
    }
    index_t.stop(by_path.len());

    // PHASE 2 — resolve imports via each language's own algorithm (through the trait).
    let resolve_t = PhaseTimer::start("resolve-imports");
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
                ResolvedImport::Resolved {
                    target, confidence, ..
                } => {
                    if target != fid {
                        graph.add_import(fid, target, confidence);
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
    resolve_t.stop(graph.imports().len());

    // Build the next extraction cache from this run's phase-1 results (move the vectors out —
    // the graph already holds everything it needs).
    let mut cache: ExtractionCache = HashMap::with_capacity(parsed.len());
    for p in parsed {
        cache.insert(
            p.rel.to_string_lossy().into_owned(),
            CachedFile {
                mtime_ns: p.mtime_ns,
                size: p.size,
                language: p.language,
                hash: p.hash,
                symbols: p.symbols,
                imports: p.imports,
                calls: p.calls,
            },
        );
    }

    Ok((graph, cache))
}

/// Reuse `w`'s cached phase-1 extraction when `prev` has a fingerprint-matching entry (so the
/// file is never read), otherwise read + parse it. A `(0, _)` mtime means metadata was
/// unavailable at walk time → never a cache hit, so we re-read rather than trust a stale entry.
fn reuse_or_parse(
    w: &Walked,
    registry: &Registry,
    prev: Option<&ExtractionCache>,
) -> Option<Parsed> {
    if w.mtime_ns != 0 {
        if let Some(cf) = prev.and_then(|p| p.get(w.rel.to_string_lossy().as_ref())) {
            if cf.mtime_ns == w.mtime_ns && cf.size == w.size {
                return Some(Parsed {
                    rel: w.rel.clone(),
                    language: cf.language.clone(),
                    hash: cf.hash,
                    symbols: cf.symbols.clone(),
                    imports: cf.imports.clone(),
                    calls: cf.calls.clone(),
                    mtime_ns: w.mtime_ns,
                    size: w.size,
                });
            }
        }
    }
    parse_one(w, registry)
}

/// A wall-clock timer for one indexing phase, printed to stderr only when `COMPASS_TIMING` is
/// set (so it's free in normal runs). `stop` reports the elapsed time and a phase-specific
/// count (files, symbols, edges) — enough to see *where* a large index spends its time.
struct PhaseTimer {
    label: &'static str,
    start: Option<std::time::Instant>,
}

impl PhaseTimer {
    fn start(label: &'static str) -> Self {
        let start = std::env::var_os("COMPASS_TIMING").map(|_| std::time::Instant::now());
        PhaseTimer { label, start }
    }
    fn stop(self, count: usize) {
        if let Some(start) = self.start {
            eprintln!(
                "compass-timing: {:<16} {:>10.3?}  ({count})",
                self.label,
                start.elapsed()
            );
        }
    }
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
        mtime_ns: w.mtime_ns,
        size: w.size,
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
            // A same-file hit is deterministic (Resolved); a unique-global hit is a
            // name-based guess (Heuristic) — correct in practice but not provable here.
            let target = local
                .get(call.callee.as_str())
                .copied()
                .map(|id| (id, EdgeConfidence::Resolved))
                .or_else(|| match by_name.get(call.callee.as_str()) {
                    Some(matches) if matches.len() == 1 => {
                        Some((matches[0], EdgeConfidence::Heuristic))
                    }
                    _ => None,
                });
            if let Some((callee, confidence)) = target {
                if callee != caller {
                    graph.add_call(caller, callee, confidence);
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
    fn all_files(&self) -> Vec<&Path> {
        self.by_path.keys().map(PathBuf::as_path).collect()
    }
}
