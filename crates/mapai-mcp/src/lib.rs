//! `mapai-mcp` — the MCP server surface (stdio, via `rmcp`).
//!
//! Depends only on `mapai-core`'s [`MapQuery`] port, never on the engine (architecture
//! §4). The CLI composition root builds the graph and hands it in as a query handle.

use std::sync::Arc;

use mapai_core::MapQuery;
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};

/// A read-only query handle the server answers tools from. `mapai-core::Graph` satisfies it.
pub type Query = Arc<dyn MapQuery + Send + Sync>;

/// The MCP server. Holds a query handle and exposes the map as MCP tools.
#[derive(Clone)]
pub struct MapServer {
    query: Query,
}

impl MapServer {
    pub fn new(query: Query) -> Self {
        Self { query }
    }
}

#[tool_router]
impl MapServer {
    #[tool(
        description = "Summary of the repository map: file, symbol, and import counts \
                          plus a per-language file breakdown."
    )]
    async fn overview(&self) -> String {
        let overview = self.query.overview();
        serde_json::to_string_pretty(&overview)
            .unwrap_or_else(|e| format!("{{\"error\":\"failed to serialize overview: {e}\"}}"))
    }
}

#[tool_handler]
impl ServerHandler for MapServer {}

/// Serve the map over MCP on stdio until the client disconnects. Builds and owns its own
/// async runtime, so callers (the CLI) stay synchronous.
pub fn serve_stdio(query: Query) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let service = MapServer::new(query)
            .serve(rmcp::transport::stdio())
            .await?;
        service.waiting().await?;
        Ok::<(), anyhow::Error>(())
    })
}
