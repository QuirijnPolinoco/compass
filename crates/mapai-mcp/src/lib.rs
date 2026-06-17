//! `mapai-mcp` — the MCP server surface. See `docs/architecture/02-architecture.md` §4.
//!
//! Exposes the map over MCP (stdio) by depending only on `mapai-core`'s query port, so
//! it never touches the engine. Implemented after `rmcp` is wired.
