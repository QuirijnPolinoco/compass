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

/// Arguments for `subgraph`: a file to center on and how far out to reach.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SubgraphArgs {
    /// Repo-relative, forward-slash path to center the slice on.
    file: String,
    /// How many import-hops out to include. Defaults to 1 (direct neighbors).
    #[serde(default)]
    depth: Option<usize>,
}

/// Arguments for `shortest_path`: the two files to connect.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct PathArgs {
    /// Repo-relative, forward-slash path to start from.
    from: String,
    /// Repo-relative, forward-slash path to reach.
    to: String,
}

/// Arguments for `get_community`: which structural community to list.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct CommunityArgs {
    /// The community id, as reported by `graph_stats`/`hubs` or shown on the visual map.
    community: u32,
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

    #[tool(
        description = "The neighborhood around a file: every file within `depth` import-hops \
                       (its dependencies and dependents) plus the import edges among them. Fetch \
                       this to load just the relevant slice of the repo instead of grepping or \
                       reading everything. Args: `file` (repo-relative path), optional `depth` \
                       (default 1)."
    )]
    async fn subgraph(
        &self,
        Parameters(SubgraphArgs { file, depth }): Parameters<SubgraphArgs>,
    ) -> String {
        let depth = depth.unwrap_or(1);
        match self.query.subgraph(&file, depth) {
            Some(sub) => serde_json::to_string_pretty(&sub).unwrap_or_default(),
            None => format!("{{\"error\":\"file not in map: {file}\"}}"),
        }
    }

    #[tool(
        description = "The shortest import path connecting two files — \"what connects X to Y\". \
                       Args: `from` and `to` are repo-relative paths. Returns the chain of files, \
                       or an error if either is unmapped or they are not connected."
    )]
    async fn shortest_path(
        &self,
        Parameters(PathArgs { from, to }): Parameters<PathArgs>,
    ) -> String {
        match self.query.shortest_path(&from, &to) {
            Some(path) => serde_json::to_string_pretty(&serde_json::json!({
                "from": from,
                "to": to,
                "path": path,
            }))
            .unwrap_or_default(),
            None => format!(
                "{{\"error\":\"no path between {from} and {to} \
                 (one may be unmapped, or they are unconnected)\"}}"
            ),
        }
    }

    #[tool(
        description = "High-level repository stats: file, symbol, and import/call edge counts \
                       (split into resolved vs heuristic), the number of structural communities \
                       and bridging hubs, a per-language breakdown, and the most-connected files. \
                       A cheap first read before deeper queries."
    )]
    async fn graph_stats(&self) -> String {
        let stats = self.query.graph_stats();
        serde_json::to_string_pretty(&stats)
            .unwrap_or_else(|e| format!("{{\"error\":\"failed to serialize graph_stats: {e}\"}}"))
    }

    #[tool(
        description = "The files that bridge many communities — shared hubs / \"god files\". \
                       These are good entry points for understanding the architecture. Each \
                       reports how many communities it bridges and its import degree."
    )]
    async fn hubs(&self) -> String {
        serde_json::to_string_pretty(&self.query.hubs())
            .unwrap_or_else(|e| format!("{{\"error\":\"failed to serialize hubs: {e}\"}}"))
    }

    #[tool(
        description = "List the files in one community — a cohesive sub-part of the repo. Arg \
                       `community` is a community id from `graph_stats`/`hubs` or the visual map. \
                       Returns an error if the id is unknown."
    )]
    async fn get_community(
        &self,
        Parameters(CommunityArgs { community }): Parameters<CommunityArgs>,
    ) -> String {
        match self.query.community(community) {
            Some(view) => serde_json::to_string_pretty(&view).unwrap_or_default(),
            None => format!("{{\"error\":\"no community with id {community}\"}}"),
        }
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
