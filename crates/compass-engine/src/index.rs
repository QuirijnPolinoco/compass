//! Full-index orchestration: walk → (parallel) parse+extract → assemble → resolve.
//! See architecture §6 Flow A. The two-phase extractor keeps per-language resolution
//! logic out of this engine (ADR-0002).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use compass_core::{
    Diagnostic, DiagnosticKind, EdgeConfidence, FileCategory, FileId, Graph, LanguageId, SymbolId,
};
use compass_extract::{
    ExtractedSymbol, LangConfig, Parsing, RawCall, RawImport, Registry, ResolutionContext,
    ResolvedImport,
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
    /// Why the contents were skipped, if they were (see [`skip_reason`]).
    not_analysed: Option<String>,
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
    /// Why the contents were skipped, if they were. Absent in older caches → analysed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_analysed: Option<String>,
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
    // A file's category is a property of the extractor that mapped it (ADR-0007), so it is
    // looked up rather than cached with the extraction.
    let categories: HashMap<LanguageId, FileCategory> = registry
        .extractors()
        .iter()
        .map(|e| (e.language_id(), e.category()))
        .collect();
    for p in &parsed {
        let category = categories.get(&p.language).cloned().unwrap_or_default();
        let fid = graph.add_file_in(p.rel.clone(), p.language.clone(), category, p.hash);
        if let Some(reason) = &p.not_analysed {
            graph.mark_not_analysed(fid, reason.clone());
        }
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
    resolve_calls(&mut graph, &parsed, &symbol_ids, registry);
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
        // By language, not by re-detecting the path: a shebang-detected file has no extension
        // to detect it by.
        let Some(extractor) = registry.extractor_for(&p.language) else {
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
                not_analysed: p.not_analysed,
            },
        );
    }

    Ok((graph, cache))
}

/// Reuse `w`'s cached phase-1 extraction when `prev` has a fingerprint-matching entry (so the
/// file is never read), otherwise read + parse it. The entry must also belong to the language
/// this registry detects for the file: a build without that language (or one that now assigns
/// the extension elsewhere) must not resurrect an extraction it could not have produced. A `(0, _)` mtime means metadata was
/// unavailable at walk time → never a cache hit, so we re-read rather than trust a stale entry.
fn reuse_or_parse(
    w: &Walked,
    registry: &Registry,
    prev: Option<&ExtractionCache>,
) -> Option<Parsed> {
    if w.mtime_ns != 0 {
        if let Some(cf) = prev.and_then(|p| p.get(w.rel.to_string_lossy().as_ref())) {
            // A file that can only be identified by its `#!` line is trusted to still be what
            // it was (its fingerprint is unchanged) as long as this build has that language.
            let same_language = if registry.needs_first_line(&w.rel) {
                registry.extractor_for(&cf.language).is_some()
            } else {
                registry
                    .detect(&w.rel, None)
                    .is_some_and(|e| e.language_id() == cf.language)
            };
            if same_language && cf.mtime_ns == w.mtime_ns && cf.size == w.size {
                return Some(Parsed {
                    rel: w.rel.clone(),
                    language: cf.language.clone(),
                    hash: cf.hash,
                    symbols: cf.symbols.clone(),
                    imports: cf.imports.clone(),
                    calls: cf.calls.clone(),
                    not_analysed: cf.not_analysed.clone(),
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

/// The first line of a file, read from its first few hundred bytes only — enough for a `#!`
/// line, and cheap enough to do for every extensionless file (`LICENSE`, `Makefile`, `bin/*`).
fn first_line(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut head = [0u8; 256];
    let read = std::fs::File::open(path).ok()?.read(&mut head).ok()?;
    let text = String::from_utf8_lossy(&head[..read]);
    text.lines().next().map(str::to_string)
}

/// Largest file whose contents are analysed. Source files people write are far smaller; what
/// exceeds this is generated, vendored or data, and parsing it costs seconds and floods the map
/// with thousands of meaningless symbols. Fixed rather than configurable: zero config is the
/// product (ADR-0007 §3).
const MAX_ANALYSED_BYTES: u64 = 1024 * 1024;

/// A line this long, on average, is not something a person wrote.
const MINIFIED_AVG_LINE_BYTES: usize = 500;
/// Don't judge tiny files by their line length (a one-line JSON config is fine).
const MINIFIED_MIN_BYTES: usize = 20 * 1024;

/// Lockfiles: machine-written, huge, and never the answer to a navigation question.
const LOCKFILES: [&str; 9] = [
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "Cargo.lock",
    "composer.lock",
    "Gemfile.lock",
    "poetry.lock",
    "go.sum",
];

/// Why a file that *is* a mapped type should not have its contents analysed — judged from its
/// name and size alone, before it is read. Language-agnostic: these are conventions of how
/// files get generated, not of any language. `whole_file` is false for extractors that only
/// read the top of a file ([`Parsing::Head`]), which makes its size irrelevant.
fn skip_reason(w: &Walked, whole_file: bool) -> Option<String> {
    let name = w.rel.file_name()?.to_string_lossy();
    if LOCKFILES.contains(&name.as_ref()) {
        return Some("lockfile".to_string());
    }
    // `app.min.js`, `site.min.css`, `vendor.bundle.min.js`.
    if name.contains(".min.") {
        return Some("minified".to_string());
    }
    (whole_file && w.size > MAX_ANALYSED_BYTES).then(|| format!("too large ({} KB)", w.size / 1024))
}

/// The first `max_bytes` of a file (fewer if it is shorter).
fn read_head(path: &Path, max_bytes: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut head = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(max_bytes as u64)
        .read_to_end(&mut head)
        .ok()?;
    Some(head)
}

/// Minified content under an ordinary name (`cytoscape.js` that is one 400 KB line).
fn looks_minified(bytes: &[u8]) -> bool {
    if bytes.len() < MINIFIED_MIN_BYTES {
        return false;
    }
    let lines = bytes.iter().filter(|&&b| b == b'\n').count() + 1;
    bytes.len() / lines > MINIFIED_AVG_LINE_BYTES
}

fn parse_one(w: &Walked, registry: &Registry) -> Option<Parsed> {
    let extractor = if registry.needs_first_line(&w.rel) {
        registry.detect(&w.rel, first_line(&w.abs).as_deref())?
    } else {
        registry.detect(&w.rel, None)?
    };
    // Still a node — so an import of it resolves to a real file — but with nothing inside.
    let skipped = |reason: String, hash: u64| Parsed {
        rel: w.rel.clone(),
        language: extractor.language_id(),
        hash,
        symbols: Vec::new(),
        imports: Vec::new(),
        calls: Vec::new(),
        not_analysed: Some(reason),
        mtime_ns: w.mtime_ns,
        size: w.size,
    };
    // When the file isn't read in full, its fingerprint stands in for a content hash.
    let fingerprint = w.mtime_ns ^ w.size;
    let parsing = extractor.parsing();
    if let Some(reason) = skip_reason(w, matches!(parsing, Parsing::Grammar(_))) {
        return Some(skipped(reason, fingerprint));
    }

    let (extraction, hash) = match parsing {
        Parsing::Head { max_bytes } => {
            let head = read_head(&w.abs, max_bytes)?;
            (extractor.extract_head(&head), fingerprint)
        }
        Parsing::Grammar(grammar) => {
            let bytes = std::fs::read(&w.abs).ok()?;
            if looks_minified(&bytes) {
                return Some(skipped("minified".to_string(), content_hash(&bytes)));
            }
            let tree = compass_extract::parse(&grammar, &bytes)?;
            (extractor.extract(&bytes, &tree), content_hash(&bytes))
        }
    };
    Some(Parsed {
        rel: w.rel.clone(),
        language: extractor.language_id(),
        hash,
        symbols: extraction.symbols,
        imports: extraction.imports,
        calls: extraction.calls,
        not_analysed: None,
        mtime_ns: w.mtime_ns,
        size: w.size,
    })
}

/// Turn raw caller/callee names into `Calls` edges. Conservative by design: a call resolves to
/// the same-file symbol of that name first, otherwise to a *unique* match within the caller's
/// [call namespace](compass_extract::Extractor::call_namespace) — names that occur in more than
/// one file (and aren't local) are left unresolved rather than guessed.
///
/// The namespace scoping matters in both directions. Without it a Python `build()` could link to
/// a lone Go `build`, and — worse, because it is silent — adding a file in *any* language that
/// defines an already-unique name would make that name ambiguous and delete a correct edge.
fn resolve_calls(
    graph: &mut Graph,
    parsed: &[Parsed],
    symbol_ids: &[Vec<SymbolId>],
    registry: &Registry,
) {
    let namespaces: HashMap<LanguageId, String> = registry
        .extractors()
        .iter()
        .map(|e| (e.language_id(), e.call_namespace()))
        .collect();
    let namespace_of = |p: &Parsed| namespaces.get(&p.language).map(String::as_str);

    // (namespace, name) → symbol ids, across the whole repo.
    let mut by_name: HashMap<(&str, &str), Vec<SymbolId>> = HashMap::new();
    for (pi, p) in parsed.iter().enumerate() {
        let Some(namespace) = namespace_of(p) else {
            continue;
        };
        for (si, s) in p.symbols.iter().enumerate() {
            by_name
                .entry((namespace, s.name.as_str()))
                .or_default()
                .push(symbol_ids[pi][si]);
        }
    }

    for (pi, p) in parsed.iter().enumerate() {
        if p.calls.is_empty() {
            continue;
        }
        let namespace = namespace_of(p);
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
            // A same-file hit is deterministic (Resolved); a unique hit elsewhere in the
            // namespace is a name-based guess (Heuristic) — correct in practice but not
            // provable here.
            let target = local
                .get(call.callee.as_str())
                .copied()
                .map(|id| (id, EdgeConfidence::Resolved))
                .or_else(|| {
                    let matches = by_name.get(&(namespace?, call.callee.as_str()))?;
                    (matches.len() == 1).then(|| (matches[0], EdgeConfidence::Heuristic))
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
