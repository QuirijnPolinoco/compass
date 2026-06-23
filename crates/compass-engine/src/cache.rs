//! `.compass/` persistence. The on-disk form is a **versioned** compatibility surface
//! (ADR-0004): on a version mismatch we discard and reindex rather than trust it.

use std::path::Path;

use compass_core::Graph;
use serde::{Deserialize, Serialize};

use crate::index::ExtractionCache;

/// Bump on any breaking change to a serialized `compass-core`/`compass-extract` type ⇒ stale
/// caches reindex. v2: `Graph` gained `calls` edges and the per-file extraction cache landed.
pub const CACHE_FORMAT_VERSION: u32 = 2;

const CACHE_DIR: &str = ".compass";
const CACHE_FILE: &str = "graph.json";
const EXTRACTIONS_FILE: &str = "extractions.json";

#[derive(Serialize)]
struct CacheOut<'a> {
    version: u32,
    graph: &'a Graph,
}

#[derive(Deserialize)]
struct CacheIn {
    version: u32,
    graph: Graph,
}

/// Write the graph to `<repo_root>/.compass/graph.json`.
pub fn save(repo_root: &Path, graph: &Graph) -> anyhow::Result<()> {
    let dir = repo_root.join(CACHE_DIR);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec_pretty(&CacheOut {
        version: CACHE_FORMAT_VERSION,
        graph,
    })?;
    std::fs::write(dir.join(CACHE_FILE), json)?;
    Ok(())
}

/// Load the cached graph, or `None` if absent, unreadable, or a stale format version
/// (caller should then reindex). Transient indices are rebuilt before returning.
pub fn load(repo_root: &Path) -> Option<Graph> {
    let path = repo_root.join(CACHE_DIR).join(CACHE_FILE);
    let bytes = std::fs::read(path).ok()?;
    let parsed: CacheIn = serde_json::from_slice(&bytes).ok()?;
    if parsed.version != CACHE_FORMAT_VERSION {
        return None; // stale format → reindex
    }
    let mut graph = parsed.graph;
    graph.reindex();
    Some(graph)
}

#[derive(Serialize)]
struct ExtractionsOut<'a> {
    version: u32,
    extractions: &'a ExtractionCache,
}

#[derive(Deserialize)]
struct ExtractionsIn {
    version: u32,
    extractions: ExtractionCache,
}

/// Persist the per-file extraction cache to `<repo_root>/.compass/extractions.json`, so the next
/// index can skip re-reading unchanged files (see [`crate::index::index_incremental`]).
pub fn save_extractions(repo_root: &Path, extractions: &ExtractionCache) -> anyhow::Result<()> {
    let dir = repo_root.join(CACHE_DIR);
    std::fs::create_dir_all(&dir)?;
    let json = serde_json::to_vec(&ExtractionsOut {
        version: CACHE_FORMAT_VERSION,
        extractions,
    })?;
    std::fs::write(dir.join(EXTRACTIONS_FILE), json)?;
    Ok(())
}

/// Load the per-file extraction cache, or `None` if absent, unreadable, or a stale version
/// (the caller then does a full index, which is always correct — just slower).
pub fn load_extractions(repo_root: &Path) -> Option<ExtractionCache> {
    let path = repo_root.join(CACHE_DIR).join(EXTRACTIONS_FILE);
    let bytes = std::fs::read(path).ok()?;
    let parsed: ExtractionsIn = serde_json::from_slice(&bytes).ok()?;
    (parsed.version == CACHE_FORMAT_VERSION).then_some(parsed.extractions)
}
