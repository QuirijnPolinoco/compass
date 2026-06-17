//! `.mapai/` persistence. The on-disk form is a **versioned** compatibility surface
//! (ADR-0004): on a version mismatch we discard and reindex rather than trust it.

use std::path::Path;

use mapai_core::Graph;
use serde::{Deserialize, Serialize};

/// Bump on any breaking change to a serialized `mapai-core` type ⇒ stale caches reindex.
pub const CACHE_FORMAT_VERSION: u32 = 1;

const CACHE_DIR: &str = ".mapai";
const CACHE_FILE: &str = "graph.json";

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

/// Write the graph to `<repo_root>/.mapai/graph.json`.
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
