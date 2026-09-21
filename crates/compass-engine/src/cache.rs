//! `.compass/` persistence. The on-disk form is a **versioned** compatibility surface
//! (ADR-0004): on a version mismatch we discard and reindex rather than trust it.

use std::path::Path;

use compass_core::Graph;
use serde::{Deserialize, Serialize};

use crate::index::ExtractionCache;

/// Bump on any breaking change to a serialized `compass-core`/`compass-extract` type ⇒ stale
/// caches reindex. v2: `Graph` gained `calls` edges and the per-file extraction cache landed.
/// v3: import/call edges gained a trailing `EdgeConfidence` (the serialized tuple shape changed).
pub const CACHE_FORMAT_VERSION: u32 = 3;

/// The Compass release that wrote a cache file. Extractors improve between releases without the
/// on-disk *format* changing (a language starts emitting calls, a new kind of symbol, …), and an
/// unchanged file is never re-read — so a cache from another release would pin its files to that
/// release's view of them forever. A producer mismatch is therefore treated like a format
/// mismatch: discard and reindex. (Every workspace crate shares one version.)
const PRODUCER: &str = env!("CARGO_PKG_VERSION");

const CACHE_DIR: &str = ".compass";
const CACHE_FILE: &str = "graph.json";
const EXTRACTIONS_FILE: &str = "extractions.json";

#[derive(Serialize)]
struct CacheOut<'a> {
    version: u32,
    producer: &'a str,
    graph: &'a Graph,
}

#[derive(Deserialize)]
struct CacheIn {
    version: u32,
    /// Absent in caches written before the producer stamp existed → never matches.
    #[serde(default)]
    producer: String,
    graph: Graph,
}

/// Write the graph to `<repo_root>/.compass/graph.json`.
pub fn save(repo_root: &Path, graph: &Graph) -> anyhow::Result<()> {
    let dir = repo_root.join(CACHE_DIR);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(&CacheOut {
        version: CACHE_FORMAT_VERSION,
        producer: PRODUCER,
        graph,
    })?;
    std::fs::write(dir.join(CACHE_FILE), json)?;
    Ok(())
}

/// Whether a graph cache file is present at `<repo_root>/.compass/graph.json`. A cheap existence
/// check (no read/parse) — callers that need the graph itself should use [`load`]. Used by
/// `compass install --guard` to warn when the guard hook would be wired up against no map.
pub fn exists(repo_root: &Path) -> bool {
    repo_root.join(CACHE_DIR).join(CACHE_FILE).exists()
}

/// Load the cached graph, or `None` if absent, unreadable, a stale format version, or written
/// by another Compass release (caller should then reindex). Transient indices are rebuilt before returning.
pub fn load(repo_root: &Path) -> Option<Graph> {
    let path = repo_root.join(CACHE_DIR).join(CACHE_FILE);
    let bytes = std::fs::read(path).ok()?;
    let parsed: CacheIn = serde_json::from_slice(&bytes).ok()?;
    if parsed.version != CACHE_FORMAT_VERSION || parsed.producer != PRODUCER {
        return None; // stale format, or another release's view of the repo → reindex
    }
    let mut graph = parsed.graph;
    graph.reindex();
    Some(graph)
}

#[derive(Serialize)]
struct ExtractionsOut<'a> {
    version: u32,
    producer: &'a str,
    extractions: &'a ExtractionCache,
}

#[derive(Deserialize)]
struct ExtractionsIn {
    version: u32,
    #[serde(default)]
    producer: String,
    extractions: ExtractionCache,
}

/// Persist the per-file extraction cache to `<repo_root>/.compass/extractions.json`, so the next
/// index can skip re-reading unchanged files (see [`crate::index::index_incremental`]).
pub fn save_extractions(repo_root: &Path, extractions: &ExtractionCache) -> anyhow::Result<()> {
    let dir = repo_root.join(CACHE_DIR);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec(&ExtractionsOut {
        version: CACHE_FORMAT_VERSION,
        producer: PRODUCER,
        extractions,
    })?;
    std::fs::write(dir.join(EXTRACTIONS_FILE), json)?;
    Ok(())
}

/// Load the per-file extraction cache, or `None` if absent, unreadable, a stale version, or
/// written by another Compass release (the caller then does a full index, which is always
/// correct — just slower).
pub fn load_extractions(repo_root: &Path) -> Option<ExtractionCache> {
    let path = repo_root.join(CACHE_DIR).join(EXTRACTIONS_FILE);
    let bytes = std::fs::read(path).ok()?;
    let parsed: ExtractionsIn = serde_json::from_slice(&bytes).ok()?;
    (parsed.version == CACHE_FORMAT_VERSION && parsed.producer == PRODUCER)
        .then_some(parsed.extractions)
}
