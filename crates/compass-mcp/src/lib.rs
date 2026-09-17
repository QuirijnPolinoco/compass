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

/// Arguments for `find_symbol`: the name to look up.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SymbolArgs {
    /// The symbol name (or part of it), case-insensitive.
    name: String,
}

/// Arguments for `symbol_calls`: the symbol, optionally pinned to one file.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SymbolCallsArgs {
    /// The exact symbol name, as reported by `find_symbol`.
    name: String,
    /// Repo-relative, forward-slash path, to pick one definition when the name is defined in
    /// several files.
    #[serde(default)]
    file: Option<String>,
}

/// The MCP server. Holds a query handle and exposes the map as MCP tools.
#[derive(Clone)]
pub struct MapServer {
    query: Query,
    /// Every language this binary can map — a property of the build, not of the mapped repo,
    /// so the composition root passes it in (the server never sees the extractor registry).
    supported_languages: Vec<String>,
}

impl MapServer {
    pub fn new(query: Query, supported_languages: Vec<String>) -> Self {
        Self {
            query,
            supported_languages,
        }
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
        description = "Circular import dependencies: every set of files that import their way back \
                       to one another (a strongly-connected component), plus any file that imports \
                       itself. Each reports the member files, a concrete cycle path (which edge to \
                       cut), its size, and whether it rests on a convention-based (heuristic) import \
                       edge that needs verification. Empty array if the repository is acyclic."
    )]
    async fn import_cycles(&self) -> String {
        serde_json::to_string_pretty(&self.query.import_cycles())
            .unwrap_or_else(|e| format!("{{\"error\":\"failed to serialize import_cycles: {e}\"}}"))
    }

    #[tool(
        description = "Files with no resolved import edges in or out — neither importing another \
                       mapped file nor imported by one. A smell, not a defect: often legitimate \
                       entrypoints, config, generated code, or standalone scripts. A file whose only \
                       import is broken is excluded (it is reported by broken_imports instead)."
    )]
    async fn isolated_files(&self) -> String {
        serde_json::to_string_pretty(&self.query.isolated_files()).unwrap_or_else(|e| {
            format!("{{\"error\":\"failed to serialize isolated_files: {e}\"}}")
        })
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

    #[tool(
        description = "What breaks if I change this file? Every file that imports it directly or \
                       transitively, each with its distance in import-hops (1 = direct), nearest \
                       first, plus direct/total counts. Check this before a refactor or a risky \
                       edit. Arg `file` is a repo-relative path."
    )]
    async fn impact(&self, Parameters(FileArgs { file }): Parameters<FileArgs>) -> String {
        match self.query.impact(&file) {
            Some(impact) => serde_json::to_string_pretty(&impact).unwrap_or_default(),
            None => format!("{{\"error\":\"file not in map: {file}\"}}"),
        }
    }

    #[tool(
        description = "Where is this symbol defined? Finds functions, classes, methods, etc. by \
                       name (case-insensitive; exact matches first, then partial) and returns each \
                       one's kind, file and line — jump straight to a definition instead of \
                       grepping. Arg `name`. Capped at 50 results."
    )]
    async fn find_symbol(&self, Parameters(SymbolArgs { name }): Parameters<SymbolArgs>) -> String {
        serde_json::to_string_pretty(&self.query.find_symbol(&name)).unwrap_or_default()
    }

    #[tool(
        description = "Who calls this symbol, and what does it call? For every symbol named exactly \
                       `name` (optionally only in `file`), lists its callers and callees with their \
                       file, line and edge confidence (resolved = same-file, heuristic = unique \
                       name match). Only unambiguous calls are recorded, so an empty list does not \
                       prove a symbol is unused."
    )]
    async fn symbol_calls(
        &self,
        Parameters(SymbolCallsArgs { name, file }): Parameters<SymbolCallsArgs>,
    ) -> String {
        serde_json::to_string_pretty(&self.query.symbol_calls(&name, file.as_deref()))
            .unwrap_or_default()
    }

    #[tool(
        description = "The languages this Compass build can map, and which of them appear in this \
                       repository. Files in any other language are not in the map — fall back to \
                       normal search for those."
    )]
    async fn supported_languages(&self) -> String {
        let in_repo: Vec<String> = self
            .query
            .overview()
            .languages
            .into_iter()
            .map(|l| l.language.to_string())
            .collect();
        serde_json::to_string_pretty(&serde_json::json!({
            "supported": self.supported_languages,
            "in_this_repository": in_repo,
        }))
        .unwrap_or_default()
    }
}

#[tool_handler]
impl ServerHandler for MapServer {}

/// Serve the map over MCP on stdio until the client disconnects. Builds and owns its own
/// async runtime, so callers (the CLI) stay synchronous.
pub fn serve_stdio(query: Query, supported_languages: Vec<String>) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let service = MapServer::new(query, supported_languages)
            .serve(rmcp::transport::stdio())
            .await?;
        service.waiting().await?;
        Ok::<(), anyhow::Error>(())
    })
}
