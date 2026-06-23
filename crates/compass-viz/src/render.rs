//! Turn a `compass-core` [`GraphView`] into the browser's render schema (Cytoscape
//! elements JSON) and hold the embedded, offline front-end assets.
//!
//! Per ADR-0004/ADR-0005 the render schema lives here, not in `compass-core`: core stays
//! renderer-unaware and `compass-viz` owns the wire shape the browser consumes — the visual
//! analogue of how `compass-mcp` owns its MCP DTOs.

use std::collections::HashSet;

use compass_core::{EdgeConfidence, EdgeKind, GraphView, NodeKind, SymbolKind};
use serde::Serialize;

// Front-end assets, embedded so the binary is self-contained and works with no network
// (ADR-0005). Cytoscape.js is vendored (MIT — see assets/LICENSE-cytoscape.txt).
pub const INDEX_HTML: &str = include_str!("../assets/index.html");
pub const APP_JS: &str = include_str!("../assets/app.js");
pub const APP_CSS: &str = include_str!("../assets/app.css");
pub const CYTOSCAPE_JS: &str = include_str!("../assets/cytoscape.umd.min.js");

/// The full payload `GET /graph` returns: the map's current version plus its elements.
#[derive(Serialize)]
struct Payload {
    version: u64,
    elements: Elements,
}

#[derive(Serialize)]
struct Elements {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

#[derive(Serialize)]
struct Node {
    data: NodeData,
}

#[derive(Serialize)]
struct NodeData {
    id: String,
    label: String,
    /// `"file"` or `"symbol"`.
    kind: &'static str,
    path: String,
    /// Top-level folder (first path component) — for the by-folder color mode.
    folder: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    language: Option<String>,
    #[serde(rename = "symbolKind", skip_serializing_if = "Option::is_none")]
    symbol_kind: Option<&'static str>,
    /// Structural community id (ADR-0005) — the default coloring.
    group: u32,
    #[serde(rename = "isHub")]
    is_hub: bool,
    degree: usize,
}

#[derive(Serialize)]
struct Edge {
    data: EdgeData,
}

#[derive(Serialize)]
struct EdgeData {
    id: String,
    source: String,
    target: String,
    /// `"import"`, `"defines"`, or `"calls"`.
    kind: &'static str,
    /// `"Resolved"` (path-exact / same-file) or `"Heuristic"` (a convention-based guess) —
    /// the front-end renders heuristic edges dashed + faint so guesses read differently.
    confidence: &'static str,
}

fn node_kind_str(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::File => "file",
        NodeKind::Symbol => "symbol",
    }
}

fn edge_kind_str(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::Import => "import",
        EdgeKind::Defines => "defines",
        EdgeKind::Calls => "calls",
    }
}

fn edge_confidence_str(confidence: EdgeConfidence) -> &'static str {
    match confidence {
        EdgeConfidence::Resolved => "Resolved",
        EdgeConfidence::Heuristic => "Heuristic",
    }
}

fn symbol_kind_str(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Method => "method",
        SymbolKind::Class => "class",
        SymbolKind::Struct => "struct",
        SymbolKind::Interface => "interface",
        SymbolKind::Enum => "enum",
        SymbolKind::Constant => "constant",
        SymbolKind::Variable => "variable",
        SymbolKind::Module => "module",
        SymbolKind::Other => "other",
    }
}

/// Top-level folder (first path component), or `""` for a root-level file. Splits on both
/// separators so the result is stable regardless of how the path was produced.
fn top_folder(path: &str) -> String {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        parts[0].to_string()
    } else {
        String::new()
    }
}

fn elements_of(view: &GraphView) -> Elements {
    let nodes = view
        .nodes
        .iter()
        .map(|n| Node {
            data: NodeData {
                id: n.id.clone(),
                label: n.label.clone(),
                kind: node_kind_str(n.kind),
                folder: top_folder(&n.path),
                path: n.path.clone(),
                language: n.language.clone(),
                symbol_kind: n.symbol_kind.map(symbol_kind_str),
                group: n.group,
                is_hub: n.is_hub,
                degree: n.degree,
            },
        })
        .collect();

    // Content-stable, deduped edge ids so SSE reconciliation diffs correctly across updates
    // (a positional `e{i}` id would re-map to a different edge when the graph changes).
    let mut seen = HashSet::new();
    let mut edges = Vec::new();
    for e in &view.edges {
        let kind = edge_kind_str(e.kind);
        let id = format!("{}|{}|{}", e.source, kind, e.target);
        if seen.insert(id.clone()) {
            edges.push(Edge {
                data: EdgeData {
                    id,
                    source: e.source.clone(),
                    target: e.target.clone(),
                    kind,
                    confidence: edge_confidence_str(e.confidence),
                },
            });
        }
    }

    Elements { nodes, edges }
}

/// Serialize a view as the `GET /graph` payload (Cytoscape elements + version).
pub fn graph_payload_json(view: &GraphView, version: u64) -> String {
    let payload = Payload {
        version,
        elements: elements_of(view),
    };
    serde_json::to_string(&payload)
        .unwrap_or_else(|_| "{\"version\":0,\"elements\":{\"nodes\":[],\"edges\":[]}}".to_string())
}

/// A single self-contained HTML file with the app, the renderer, and both graph views
/// inlined — opens offline with no server (`compass map --snapshot`, ADR-0005).
pub fn snapshot_html(files_view: &GraphView, symbols_view: &GraphView) -> String {
    let files = serde_json::to_string(&elements_of(files_view)).unwrap_or_default();
    let symbols = serde_json::to_string(&elements_of(symbols_view)).unwrap_or_default();
    // The app reads window.__COMPASS__ when present (snapshot mode) instead of fetching.
    let bootstrap =
        format!("<script>window.__COMPASS__={{files:{files},symbols:{symbols}}};</script>");
    INDEX_HTML
        .replace(
            "<link rel=\"stylesheet\" href=\"/app.css\">",
            &format!("<style>{APP_CSS}</style>"),
        )
        .replace(
            "<script src=\"/vendor/cytoscape.min.js\"></script>",
            &format!("<script>{CYTOSCAPE_JS}</script>"),
        )
        .replace(
            "<script src=\"/app.js\"></script>",
            &format!("{bootstrap}<script>{APP_JS}</script>"),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use compass_core::{EdgeConfidence, GraphEdge, GraphNode};

    fn file_node(path: &str, group: u32) -> GraphNode {
        GraphNode {
            id: path.to_string(),
            label: path.rsplit('/').next().unwrap().to_string(),
            kind: NodeKind::File,
            path: path.to_string(),
            language: Some("rust".to_string()),
            symbol_kind: None,
            group,
            is_hub: false,
            degree: 1,
        }
    }

    #[test]
    fn top_folder_cases() {
        assert_eq!(top_folder("src/auth/login.rs"), "src");
        assert_eq!(top_folder("main.rs"), "");
        assert_eq!(top_folder("crates/core/lib.rs"), "crates");
        assert_eq!(top_folder("a\\b\\c.cs"), "a");
    }

    #[test]
    fn payload_has_nodes_edges_and_dedups_edges() {
        let view = GraphView {
            nodes: vec![file_node("a.rs", 0), file_node("b.rs", 0)],
            // duplicate import edge should collapse to one
            edges: vec![
                GraphEdge {
                    source: "a.rs".into(),
                    target: "b.rs".into(),
                    kind: EdgeKind::Import,
                    confidence: EdgeConfidence::Resolved,
                },
                GraphEdge {
                    source: "a.rs".into(),
                    target: "b.rs".into(),
                    kind: EdgeKind::Import,
                    confidence: EdgeConfidence::Resolved,
                },
            ],
        };
        let json = graph_payload_json(&view, 7);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["version"], 7);
        assert_eq!(parsed["elements"]["nodes"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["elements"]["edges"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["elements"]["nodes"][0]["data"]["folder"], "");
        assert_eq!(
            parsed["elements"]["edges"][0]["data"]["confidence"],
            "Resolved"
        );
    }

    #[test]
    fn heuristic_edge_confidence_is_emitted() {
        let view = GraphView {
            nodes: vec![file_node("a.rs", 0), file_node("b.rs", 0)],
            edges: vec![GraphEdge {
                source: "a.rs".into(),
                target: "b.rs".into(),
                kind: EdgeKind::Import,
                confidence: EdgeConfidence::Heuristic,
            }],
        };
        let json = graph_payload_json(&view, 1);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["elements"]["edges"][0]["data"]["confidence"],
            "Heuristic"
        );
    }

    #[test]
    fn snapshot_inlines_assets_and_data() {
        let view = GraphView {
            nodes: vec![file_node("a.rs", 0)],
            edges: vec![],
        };
        let html = snapshot_html(&view, &view);
        assert!(html.contains("window.__COMPASS__"));
        // assets are inlined, not referenced
        assert!(!html.contains("src=\"/app.js\""));
        assert!(!html.contains("href=\"/app.css\""));
    }
}
