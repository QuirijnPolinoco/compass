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

use compass_core::{GraphView, MapQuery, Subgraph};

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
