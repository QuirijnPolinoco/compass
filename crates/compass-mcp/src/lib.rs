//! `compass-mcp` — the MCP server surface (stdio, via `rmcp`).
//!
//! Depends only on `compass-core`'s [`MapQuery`] port, never on the engine (architecture
//! §4). The CLI composition root builds the graph and hands it in as a query handle.

use std::sync::Arc;

use compass_core::MapQuery;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::Deserialize;

/// A read-only query handle the server answers tools from. `compass-core::Graph` satisfies it.
pub type Query = Arc<dyn MapQuery + Send + Sync>;

/// Arguments for tools that operate on a single file.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct FileArgs {
    /// Repo-relative, forward-slash path of the file (as shown in the map).
    file: String,
}

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

    #[tool(
        description = "What a file imports and what imports it. Argument `file` is a \
                       repo-relative path exactly as shown in the map."
    )]
    async fn file_dependencies(
        &self,
        Parameters(FileArgs { file }): Parameters<FileArgs>,
    ) -> String {
        match self.query.file_dependencies(&file) {
            Some(deps) => serde_json::to_string_pretty(&deps).unwrap_or_default(),
            None => format!("{{\"error\":\"file not in map: {file}\"}}"),
        }
    }

    #[tool(description = "List imports that resolve to no real file (broken references).")]
    async fn broken_imports(&self) -> String {
        serde_json::to_string_pretty(&self.query.broken_imports()).unwrap_or_default()
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
