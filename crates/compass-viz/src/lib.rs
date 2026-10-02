//! `compass-viz` — the interactive **visual map** surface (ADR-0005).
//!
//! A second protocol surface alongside `compass-mcp`: it consumes only `compass-core`'s
//! [`MapQuery`] port (never the engine), serves a force-directed graph to the browser over a
//! `127.0.0.1` HTTP+SSE server, and pushes live updates as the map changes. The renderer
//! (Cytoscape.js) and front-end assets are embedded, so the binary works fully offline.
//!
//! The CLI composition root wires the concrete engine/graph in as the [`Query`] handle and
//! republishes a fresh one on every watch event via [`MapState::publish`].

mod render;
mod server;
mod session_tokens;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use compass_core::{ContextPack, ContextRequest, GraphView, MapQuery, Subgraph};

pub use server::{bind, VizServer};
pub use session_tokens::{aggregate_session_tokens, SessionTokenSummary, SessionTokens};

/// The uncommon high default port (ADR-0005): clear of typical dev servers, databases, and
/// container/registry ports, so it won't collide with the user's other work. If it's busy,
/// [`bind`] falls back to an OS-assigned free port.
pub const DEFAULT_PORT: u16 = 62049;

/// A read-only query handle the viz answers from. `compass-core::Graph` satisfies it — the
/// same port `compass-mcp` uses.
pub type Query = Arc<dyn MapQuery + Send + Sync>;

struct Inner {
    query: Query,
    version: u64,
}

/// Shared, swappable map state behind the server. The CLI calls [`publish`](Self::publish)
/// with a freshly-indexed graph on each change; connected SSE clients are woken and refetch.
pub struct MapState {
    inner: Mutex<Inner>,
    changed: Condvar,
    /// Repo root the map was indexed from, so read-only local routes (the token-savings
    /// dashboard) can read `<repo>/.compass/sessions/`. Never written; loopback + read-only.
    repo_root: PathBuf,
}

impl MapState {
    /// Create state seeded with the initial map and the repo root it was indexed from.
    /// Snapshot mode and tests can pass `"."` or any path; only the `/tokens` routes read it.
    pub fn new(query: Query, repo_root: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner { query, version: 0 }),
            changed: Condvar::new(),
            repo_root,
        })
    }

    /// Repo root for the read-only local token-savings routes.
    pub(crate) fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Replace the map with a freshly-indexed one and wake every open SSE stream so the
    /// browser refetches and the picture updates in place (ADR-0005, Flow D).
    pub fn publish(&self, query: Query) {
        {
            let mut inner = self.inner.lock().unwrap();
            inner.query = query;
            inner.version += 1;
        }
        self.changed.notify_all();
    }

    /// Current map version (bumped on every [`publish`](Self::publish)).
    pub(crate) fn version(&self) -> u64 {
        self.inner.lock().unwrap().version
    }

    pub(crate) fn graph_view(&self, include_symbols: bool) -> GraphView {
        self.inner.lock().unwrap().query.graph_view(include_symbols)
    }

    pub(crate) fn subgraph(&self, file: &str, depth: usize) -> Option<Subgraph> {
        self.inner.lock().unwrap().query.subgraph(file, depth)
    }

    /// The files a free-text task points at, ranked the way `compass context` ranks them for an
    /// AI. Empty when the text matched nothing (never the most-connected fallback: for a search
    /// box that would look like results that are not).
    pub(crate) fn search(&self, text: &str, max_files: usize) -> ContextPack {
        let mut pack = self.inner.lock().unwrap().query.context(&ContextRequest {
            query: Some(text.to_string()),
            seeds: Vec::new(),
            max_files,
        });
        if pack.selected_by != "query" {
            pack.files.clear();
        }
        pack
    }

    /// Block until the version differs from `last`, or `timeout` elapses (for keep-alives).
    /// Returns the current version.
    pub(crate) fn wait_for_change(&self, last: u64, timeout: Duration) -> u64 {
        let inner = self.inner.lock().unwrap();
        let (inner, _) = self
            .changed
            .wait_timeout_while(inner, timeout, |inner| inner.version == last)
            .unwrap();
        inner.version
    }
}

/// Render a single self-contained HTML snapshot of the current map (`compass map
/// --snapshot`) — opens offline, no server. Both the files-only and files+symbols views are
/// inlined so the in-page toggle still works.
pub fn snapshot_html(query: &Query) -> String {
    render::snapshot_html(&query.graph_view(false), &query.graph_view(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use compass_core::{Graph, LanguageId, Span, SymbolKind};

    fn state() -> Arc<MapState> {
        let mut g = Graph::new();
        let rs = LanguageId::new("rust");
        let gate = g.add_file("src/risk/gate.rs".into(), rs.clone(), 0);
        g.add_file("src/ui/page.rs".into(), rs, 0);
        let span = Span {
            start_byte: 0,
            end_byte: 0,
            start_row: 41,
            start_col: 0,
        };
        g.add_symbol("drawdown_cap".into(), SymbolKind::Function, gate, span);
        MapState::new(Arc::new(g), PathBuf::from("."))
    }

    #[test]
    fn search_ranks_files_for_a_task_with_symbol_lines() {
        let pack = state().search("where is the drawdown cap", 8);
        assert_eq!(pack.files.len(), 1);
        assert_eq!(pack.files[0].path, "src/risk/gate.rs");
        assert_eq!(pack.files[0].symbols[0].line, 42);
    }

    #[test]
    fn search_never_pads_a_miss_with_unrelated_files() {
        assert!(state().search("kubernetes helm chart", 8).files.is_empty());
        assert!(state().search("", 8).files.is_empty());
    }
}
