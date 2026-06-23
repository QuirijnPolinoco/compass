//! `compass-core` — the language-agnostic domain model for Compass.
//!
//! Holds the graph (files, symbols, edges), the diagnostics sink, and the read-only
//! query port the MCP layer talks to. It knows nothing about MCP, tree-sitter, or any
//! specific language. See `docs/architecture/02-architecture.md` §4–§5.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Open identifier for a language (e.g. `"go"`).
///
/// Deliberately NOT an enum: languages are plugins, so adding one must never edit core
/// (the North Star, ADR-0002).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LanguageId(String);

impl LanguageId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LanguageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FileId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SymbolId(pub u32);

/// A mapped source file (graph node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct File {
    pub id: FileId,
    /// Repo-relative path, forward-slash normalized for stable cross-platform output.
    pub path: PathBuf,
    pub language: LanguageId,
    /// Hash of the file contents — drives incremental staleness detection.
    pub content_hash: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Class,
    Struct,
    Interface,
    Enum,
    Constant,
    Variable,
    Module,
    Other,
}

/// A source location (byte range + start row/col), enough to jump to a symbol.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Span {
    pub start_byte: usize,
    pub end_byte: usize,
    pub start_row: usize,
    pub start_col: usize,
}

/// A defined symbol (graph node).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub file: FileId,
    pub span: Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiagnosticKind {
    /// A file (or part of it) could not be parsed; contained, never fatal.
    ParseError,
    /// An import looked internal but resolved to no real file (FR-12/D2).
    UnresolvedImport,
}

/// A non-fatal issue. The universal sink: collected, never crashes the index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub file: FileId,
    pub message: String,
}

/// The map: nodes + edges + diagnostics.
///
/// Single-writer, in-memory. Its serde form is a versioned cache surface (ADR-0004);
/// transient indices (`by_path`) are rebuilt via [`Graph::reindex`] after load.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Graph {
    files: Vec<File>,
    symbols: Vec<Symbol>,
    imports: Vec<(FileId, FileId)>,
    defines: Vec<(FileId, SymbolId)>,
    calls: Vec<(SymbolId, SymbolId)>,
    diagnostics: Vec<Diagnostic>,
    #[serde(skip)]
    by_path: HashMap<PathBuf, FileId>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_file(&mut self, path: PathBuf, language: LanguageId, content_hash: u64) -> FileId {
        let id = FileId(self.files.len() as u32);
        self.by_path.insert(path.clone(), id);
        self.files.push(File {
            id,
            path,
            language,
            content_hash,
        });
        id
    }

    pub fn add_symbol(
        &mut self,
        name: String,
        kind: SymbolKind,
        file: FileId,
        span: Span,
    ) -> SymbolId {
        let id = SymbolId(self.symbols.len() as u32);
        self.symbols.push(Symbol {
            id,
            name,
            kind,
            file,
            span,
        });
        self.defines.push((file, id));
        id
    }

    pub fn add_import(&mut self, from: FileId, to: FileId) {
        self.imports.push((from, to));
    }

    pub fn add_call(&mut self, from: SymbolId, to: SymbolId) {
        self.calls.push((from, to));
    }

    pub fn add_diagnostic(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn files(&self) -> &[File] {
        &self.files
    }

    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }

    pub fn imports(&self) -> &[(FileId, FileId)] {
        &self.imports
    }

    /// Resolved symbol→symbol call edges (caller, callee). Populated by the engine from each
    /// extractor's raw calls; only unambiguous targets become edges (see `compass-engine`).
    pub fn calls(&self) -> &[(SymbolId, SymbolId)] {
        &self.calls
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    /// Resolve a repo-relative path to its `FileId`, if mapped.
    pub fn file_id(&self, path: &Path) -> Option<FileId> {
        self.by_path.get(path).copied()
    }

    /// The repo-relative path of a file node.
    pub fn file_path(&self, id: FileId) -> Option<&Path> {
        self.files.get(id.0 as usize).map(|f| f.path.as_path())
    }

    /// Rebuild transient indices after deserializing from cache.
    pub fn reindex(&mut self) {
        self.by_path = self.files.iter().map(|f| (f.path.clone(), f.id)).collect();
    }

    /// Undirected adjacency over import edges, indexed by `FileId.0`. Self-loops dropped.
    fn import_adjacency(&self) -> Vec<Vec<FileId>> {
        let mut adj: Vec<Vec<FileId>> = vec![Vec::new(); self.files.len()];
        for &(a, b) in &self.imports {
            if a == b {
                continue;
            }
            adj[a.0 as usize].push(b);
            adj[b.0 as usize].push(a);
        }
        adj
    }

    /// Deterministic community detection over the import graph (ADR-0005), used to color the
    /// visual map by "sub-part of the project". Uses **Louvain** (modularity maximization),
    /// which — unlike label propagation — does not collapse distinct clusters that happen to be
    /// bridged by a shared file (a util imported everywhere). No RNG: nodes are visited in id
    /// order with smallest-id tie-breaks, so the result (and the map's colors) is stable.
    ///
    /// Returns, per `FileId.0`: a compacted group id (0..k by first appearance) and whether the
    /// file is a **hub** — a connector whose neighbors span ≥ 3 distinct communities.
    fn detect_communities(&self) -> (Vec<u32>, Vec<bool>) {
        let n = self.files.len();
        let adj = self.import_adjacency();

        // Unit-weighted adjacency for Louvain (parallel imports sum into a heavier edge).
        let weighted: Vec<Vec<(usize, f64)>> = adj
            .iter()
            .map(|nbrs| nbrs.iter().map(|f| (f.0 as usize, 1.0)).collect())
            .collect();
        let groups = louvain_communities(&weighted);

        // A hub connects three or more distinct communities (e.g. a shared util).
        let is_hub: Vec<bool> = (0..n)
            .map(|i| {
                adj[i]
                    .iter()
                    .map(|nb| groups[nb.0 as usize])
                    .collect::<HashSet<u32>>()
                    .len()
                    >= 3
            })
            .collect();

        (groups, is_hub)
    }
}

/// Deterministic multi-level **Louvain** community detection on a weighted, undirected graph
/// (adjacency stored both directions, no input self-loops). Returns a community id per node,
/// compacted to `0..k` by first appearance. See ADR-0005.
fn louvain_communities(adj0: &[Vec<(usize, f64)>]) -> Vec<u32> {
    let n0 = adj0.len();
    if n0 == 0 {
        return Vec::new();
    }

    // Original node -> its community at the current level (composed across levels).
    let mut node_to_comm: Vec<usize> = (0..n0).collect();

    // Working graph as summed weighted adjacency (gains self-loops as we aggregate).
    let mut graph: Vec<HashMap<usize, f64>> = adj0
        .iter()
        .map(|nbrs| {
            let mut m: HashMap<usize, f64> = HashMap::new();
            for &(j, w) in nbrs {
                *m.entry(j).or_insert(0.0) += w;
            }
            m
        })
        .collect();

    loop {
        let comm = louvain_one_level(&graph);
        let n_comm = comm.iter().copied().max().map(|c| c + 1).unwrap_or(0);
        if n_comm == graph.len() {
            break; // no node moved → converged
        }
        // Fold this level's assignment into the original-node mapping.
        for c in node_to_comm.iter_mut() {
            *c = comm[*c];
        }
        // Aggregate: one super-node per community; intra-community edges become self-loops.
        let mut next: Vec<HashMap<usize, f64>> = vec![HashMap::new(); n_comm];
        for (i, nbrs) in graph.iter().enumerate() {
            let ci = comm[i];
            for (&j, &w) in nbrs {
                *next[ci].entry(comm[j]).or_insert(0.0) += w;
            }
        }
        graph = next;
        if graph.len() == 1 {
            break;
        }
    }

    // Compact original-node communities to 0..k by first appearance (deterministic).
    let mut remap: HashMap<usize, u32> = HashMap::new();
    node_to_comm
        .iter()
        .map(|&c| {
            let next = remap.len() as u32;
            *remap.entry(c).or_insert(next)
        })
        .collect()
}

/// One level of Louvain local-moving: starting from every node in its own community, greedily
/// move each node into the neighboring community giving the largest modularity gain, until no
/// move helps. Returns the community per node, compacted to `0..k`. Deterministic.
fn louvain_one_level(graph: &[HashMap<usize, f64>]) -> Vec<usize> {
    let n = graph.len();
    // Node degree = sum of incident weights (a self-loop counts twice, as its weight appears
    // once in the map but represents both endpoints).
    let deg: Vec<f64> = graph.iter().map(|m| m.values().sum()).collect();
    let two_m: f64 = deg.iter().sum();
    if two_m == 0.0 {
        return (0..n).collect();
    }

    let mut comm: Vec<usize> = (0..n).collect();
    let mut sigma_tot: Vec<f64> = deg.clone(); // Σ degree of nodes currently in community c

    loop {
        let mut moved = false;
        for i in 0..n {
            let ci = comm[i];
            let ki = deg[i];

            // Weight from i into each neighboring community (excluding i's self-loop).
            let mut k_i_in: HashMap<usize, f64> = HashMap::new();
            for (&j, &w) in &graph[i] {
                if j != i {
                    *k_i_in.entry(comm[j]).or_insert(0.0) += w;
                }
            }

            // Tentatively remove i from its community.
            sigma_tot[ci] -= ki;

            // Baseline: staying in ci. Gain ∝ k_i_in[c] - Σtot[c]·k_i / 2m.
            let mut best_c = ci;
            let mut best_gain =
                k_i_in.get(&ci).copied().unwrap_or(0.0) - sigma_tot[ci] * ki / two_m;

            let mut candidates: Vec<usize> = k_i_in.keys().copied().collect();
            candidates.sort_unstable(); // deterministic visit order
            for c in candidates {
                let gain = k_i_in[&c] - sigma_tot[c] * ki / two_m;
                if gain > best_gain + 1e-12 {
                    best_gain = gain;
                    best_c = c;
                }
            }

            // Commit i into the chosen community.
            sigma_tot[best_c] += ki;
            if best_c != ci {
                comm[i] = best_c;
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }

    // Compact community ids to 0..k by first appearance.
    let mut remap: HashMap<usize, usize> = HashMap::new();
    comm.iter()
        .map(|&c| {
            let next = remap.len();
            *remap.entry(c).or_insert(next)
        })
        .collect()
}

/// Read-only query port the MCP and viz layers depend on, so they never touch the engine
/// (ADR-0002 / architecture §4). The concrete graph implements it.
pub trait MapQuery {
    fn overview(&self) -> Overview;
    /// What `file` imports, and what imports it (FR-10/B2). `file` is a repo-relative,
    /// forward-slash path. Returns `None` if the file isn't in the map.
    fn file_dependencies(&self, file: &str) -> Option<FileDependencies>;
    /// Imports that point at no real file — mistakes to catch early (FR-12/D2).
    fn broken_imports(&self) -> Vec<BrokenImport>;
    /// The whole map as renderable nodes + edges for the visual map (FR-20, ADR-0005).
    /// File nodes carry a structural community `group` and an `is_hub` flag. With
    /// `include_symbols`, symbol nodes + `Defines`/`Calls` edges are included too.
    fn graph_view(&self, include_symbols: bool) -> GraphView;
    /// The neighborhood within `depth` import-hops of `file` — a small, cheap slice for
    /// the AI to fetch instead of grepping (FR-11/C3). `None` if the file isn't mapped.
    fn subgraph(&self, file: &str, depth: usize) -> Option<Subgraph>;
    /// The shortest import path connecting `from` to `to` (treated as undirected) —
    /// "what connects X to Y" (FR-17/E1). `None` if either is unmapped or unconnected.
    fn shortest_path(&self, from: &str, to: &str) -> Option<Vec<String>>;
    /// A token-bounded slice of the map to **pre-inject** into an AI prompt so it reasons
    /// instead of exploring (ADR-0006): a structural summary plus the most relevant files
    /// (by seed-neighborhood, query-term match, or centrality), each with its symbols and
    /// imports/dependents.
    fn context(&self, request: &ContextRequest) -> ContextPack;
}

/// A high-level summary of the map (FR-3/B1, the `overview` MCP tool).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Overview {
    pub file_count: usize,
    pub symbol_count: usize,
    pub import_edge_count: usize,
    pub diagnostic_count: usize,
    pub languages: Vec<LanguageStat>,
    /// The files with the most import connections — where the important logic tends to
    /// live (FR-16/B3). Capped to the top handful.
    pub most_connected: Vec<ConnectedFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LanguageStat {
    pub language: LanguageId,
    pub file_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectedFile {
    pub file: String,
    /// Number of import edges touching this file (in + out).
    pub connections: usize,
}

/// What a file depends on and what depends on it (FR-10/B2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDependencies {
    pub file: String,
    /// Files this file imports.
    pub dependencies: Vec<String>,
    /// Files that import this file.
    pub dependents: Vec<String>,
}

/// An import that resolved to no real file (FR-12/D2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrokenImport {
    pub file: String,
    pub message: String,
}

/// What a graph node represents in the visual map (ADR-0005).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    File,
    Symbol,
}

/// What a graph edge represents in the visual map (ADR-0005).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    Import,
    Defines,
    Calls,
}

/// A renderer-neutral node for the visual map. `compass-viz` maps this to its own
/// Cytoscape element schema; nothing here is renderer-specific (ADR-0004 / ADR-0005).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    /// Stable, unique id: the repo-relative path for files, `sym:<n>` for symbols.
    pub id: String,
    /// Display label: file name (basename) or symbol name.
    pub label: String,
    pub kind: NodeKind,
    /// Repo-relative path of the file (for a symbol, its defining file).
    pub path: String,
    /// Language id (files; symbols inherit their file's), if known.
    pub language: Option<String>,
    /// Symbol kind, for symbol nodes only.
    pub symbol_kind: Option<SymbolKind>,
    /// Structural community id — files that depend on each other share one (ADR-0005).
    /// Symbols inherit their defining file's group.
    pub group: u32,
    /// True for files that connect many communities (shared hubs) — rendered neutral.
    pub is_hub: bool,
    /// Connection count, for node sizing (import degree for files; call degree for symbols).
    pub degree: usize,
}

/// A renderer-neutral edge for the visual map (ADR-0005).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub kind: EdgeKind,
}

/// The whole map as nodes + edges, ready to render (FR-20, ADR-0005).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphView {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

/// A small neighborhood around one file — the AI's cheap-to-fetch slice (FR-11/C3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subgraph {
    /// The file the neighborhood is centered on (repo-relative path).
    pub focus: String,
    /// How many import-hops out the slice reaches.
    pub depth: usize,
    /// Every file within `depth` hops of `focus` (including `focus`), sorted.
    pub files: Vec<String>,
    /// Import edges among those files, as `(from, to)` repo-relative paths, sorted.
    pub imports: Vec<(String, String)>,
}

/// What to select for a pre-injection context pack (ADR-0006).
#[derive(Debug, Clone, Default)]
pub struct ContextRequest {
    /// Free-text task/prompt; files are ranked by term matches on path + symbol names.
    pub query: Option<String>,
    /// Repo-relative files the agent is working on; their 1-hop neighborhood is selected.
    pub seeds: Vec<String>,
    /// Cap on how many files to include (keeps the pack small).
    pub max_files: usize,
}

/// One file in a context pack: enough to reason about it without opening it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextFile {
    pub path: String,
    pub language: Option<String>,
    /// Defined symbol names (capped).
    pub symbols: Vec<String>,
    /// Files this file imports.
    pub depends_on: Vec<String>,
    /// Files that import this file (capped).
    pub dependents: Vec<String>,
}

/// A token-bounded slice of the map to pre-inject into a prompt (ADR-0006).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPack {
    pub file_count: usize,
    pub languages: Vec<LanguageStat>,
    /// The most-connected files overall (a structural primer).
    pub most_connected: Vec<ConnectedFile>,
    /// The selected relevant files.
    pub files: Vec<ContextFile>,
    /// How `files` were selected ("seeds", "query", or "most-connected").
    pub selected_by: String,
}

impl MapQuery for Graph {
    fn overview(&self) -> Overview {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for f in &self.files {
            *counts.entry(f.language.as_str()).or_insert(0) += 1;
        }
        let mut languages: Vec<LanguageStat> = counts
            .into_iter()
            .map(|(l, c)| LanguageStat {
                language: LanguageId::new(l),
                file_count: c,
            })
            .collect();
        languages.sort_by(|a, b| {
            b.file_count
                .cmp(&a.file_count)
                .then_with(|| a.language.as_str().cmp(b.language.as_str()))
        });
        let mut degree: HashMap<FileId, usize> = HashMap::new();
        for (from, to) in &self.imports {
            *degree.entry(*from).or_insert(0) += 1;
            *degree.entry(*to).or_insert(0) += 1;
        }
        let mut most_connected: Vec<ConnectedFile> = degree
            .into_iter()
            .filter_map(|(id, connections)| {
                self.file_path(id).map(|p| ConnectedFile {
                    file: p.to_string_lossy().into_owned(),
                    connections,
                })
            })
            .collect();
        most_connected.sort_by(|a, b| {
            b.connections
                .cmp(&a.connections)
                .then_with(|| a.file.cmp(&b.file))
        });
        most_connected.truncate(10);

        Overview {
            file_count: self.files.len(),
            symbol_count: self.symbols.len(),
            import_edge_count: self.imports.len(),
            diagnostic_count: self.diagnostics.len(),
            languages,
            most_connected,
        }
    }

    fn file_dependencies(&self, file: &str) -> Option<FileDependencies> {
        let fid = self.file_id(Path::new(file))?;

        let mut dependencies: Vec<String> = self
            .imports
            .iter()
            .filter(|(from, _)| *from == fid)
            .filter_map(|(_, to)| self.file_path(*to))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        dependencies.sort();
        dependencies.dedup();

        let mut dependents: Vec<String> = self
            .imports
            .iter()
            .filter(|(_, to)| *to == fid)
            .filter_map(|(from, _)| self.file_path(*from))
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        dependents.sort();
        dependents.dedup();

        Some(FileDependencies {
            file: file.to_string(),
            dependencies,
            dependents,
        })
    }

    fn broken_imports(&self) -> Vec<BrokenImport> {
        self.diagnostics
            .iter()
            .filter(|d| d.kind == DiagnosticKind::UnresolvedImport)
            .filter_map(|d| {
                let file = self.file_path(d.file)?.to_string_lossy().into_owned();
                Some(BrokenImport {
                    file,
                    message: d.message.clone(),
                })
            })
            .collect()
    }

    fn graph_view(&self, include_symbols: bool) -> GraphView {
        let (groups, is_hub) = self.detect_communities();

        // File import degree (in + out), for node sizing.
        let mut file_degree: HashMap<FileId, usize> = HashMap::new();
        for &(a, b) in &self.imports {
            *file_degree.entry(a).or_insert(0) += 1;
            *file_degree.entry(b).or_insert(0) += 1;
        }

        let path_of = |id: FileId| -> Option<String> {
            self.file_path(id).map(|p| p.to_string_lossy().into_owned())
        };

        let mut nodes: Vec<GraphNode> = Vec::with_capacity(self.files.len());
        for f in &self.files {
            let idx = f.id.0 as usize;
            let path = f.path.to_string_lossy().into_owned();
            let label = f
                .path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            nodes.push(GraphNode {
                id: path.clone(),
                label,
                kind: NodeKind::File,
                path,
                language: Some(f.language.as_str().to_string()),
                symbol_kind: None,
                group: groups[idx],
                is_hub: is_hub[idx],
                degree: file_degree.get(&f.id).copied().unwrap_or(0),
            });
        }

        let mut edges: Vec<GraphEdge> = Vec::new();
        for &(a, b) in &self.imports {
            if let (Some(source), Some(target)) = (path_of(a), path_of(b)) {
                edges.push(GraphEdge {
                    source,
                    target,
                    kind: EdgeKind::Import,
                });
            }
        }

        if include_symbols {
            // Symbol call degree (in + out), for sizing.
            let mut sym_degree: HashMap<SymbolId, usize> = HashMap::new();
            for &(a, b) in &self.calls {
                *sym_degree.entry(a).or_insert(0) += 1;
                *sym_degree.entry(b).or_insert(0) += 1;
            }
            for s in &self.symbols {
                let fidx = s.file.0 as usize;
                nodes.push(GraphNode {
                    id: format!("sym:{}", s.id.0),
                    label: s.name.clone(),
                    kind: NodeKind::Symbol,
                    path: path_of(s.file).unwrap_or_default(),
                    language: self
                        .files
                        .get(fidx)
                        .map(|f| f.language.as_str().to_string()),
                    symbol_kind: Some(s.kind),
                    group: groups.get(fidx).copied().unwrap_or(0),
                    is_hub: false,
                    degree: sym_degree.get(&s.id).copied().unwrap_or(0),
                });
            }
            for &(f, s) in &self.defines {
                if let Some(source) = path_of(f) {
                    edges.push(GraphEdge {
                        source,
                        target: format!("sym:{}", s.0),
                        kind: EdgeKind::Defines,
                    });
                }
            }
            for &(a, b) in &self.calls {
                edges.push(GraphEdge {
                    source: format!("sym:{}", a.0),
                    target: format!("sym:{}", b.0),
                    kind: EdgeKind::Calls,
                });
            }
        }

        GraphView { nodes, edges }
    }

    fn subgraph(&self, file: &str, depth: usize) -> Option<Subgraph> {
        let start = self.file_id(Path::new(file))?;
        let adj = self.import_adjacency();
        let n = self.files.len();

        // BFS outward up to `depth` import-hops.
        let mut seen = vec![false; n];
        seen[start.0 as usize] = true;
        let mut frontier = vec![start];
        for _ in 0..depth {
            let mut next = Vec::new();
            for &node in &frontier {
                for &nb in &adj[node.0 as usize] {
                    if !seen[nb.0 as usize] {
                        seen[nb.0 as usize] = true;
                        next.push(nb);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }

        let mut files: Vec<String> = (0..n)
            .filter(|&i| seen[i])
            .filter_map(|i| {
                self.file_path(FileId(i as u32))
                    .map(|p| p.to_string_lossy().into_owned())
            })
            .collect();
        files.sort();

        let mut imports: Vec<(String, String)> = self
            .imports
            .iter()
            .filter(|(a, b)| seen[a.0 as usize] && seen[b.0 as usize])
            .filter_map(|(a, b)| {
                let from = self.file_path(*a)?.to_string_lossy().into_owned();
                let to = self.file_path(*b)?.to_string_lossy().into_owned();
                Some((from, to))
            })
            .collect();
        imports.sort();
        imports.dedup();

        Some(Subgraph {
            focus: file.to_string(),
            depth,
            files,
            imports,
        })
    }

    fn shortest_path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        let start = self.file_id(Path::new(from))?;
        let goal = self.file_id(Path::new(to))?;
        if start == goal {
            return Some(vec![from.to_string()]);
        }
        let adj = self.import_adjacency();
        let n = self.files.len();
        let mut seen = vec![false; n];
        let mut prev: Vec<Option<FileId>> = vec![None; n];
        let mut queue = VecDeque::new();
        seen[start.0 as usize] = true;
        queue.push_back(start);

        while let Some(node) = queue.pop_front() {
            if node == goal {
                break;
            }
            // Deterministic neighbor order so the chosen shortest path is stable.
            let mut nbs = adj[node.0 as usize].clone();
            nbs.sort();
            nbs.dedup();
            for nb in nbs {
                if !seen[nb.0 as usize] {
                    seen[nb.0 as usize] = true;
                    prev[nb.0 as usize] = Some(node);
                    queue.push_back(nb);
                }
            }
        }

        if !seen[goal.0 as usize] {
            return None;
        }

        let mut chain = vec![goal];
        let mut cur = goal;
        while let Some(p) = prev[cur.0 as usize] {
            chain.push(p);
            cur = p;
        }
        chain.reverse();
        Some(
            chain
                .iter()
                .filter_map(|id| {
                    self.file_path(*id)
                        .map(|p| p.to_string_lossy().into_owned())
                })
                .collect(),
        )
    }

    fn context(&self, request: &ContextRequest) -> ContextPack {
        let max = request.max_files.max(1);
        let (ids, selected_by) = if !request.seeds.is_empty() {
            (self.context_by_seeds(&request.seeds, max), "seeds")
        } else if let Some(q) = request.query.as_deref().filter(|q| !q.trim().is_empty()) {
            let ranked = self.context_by_query(q, max);
            if ranked.is_empty() {
                (self.most_connected_ids(max), "most-connected")
            } else {
                (ranked, "query")
            }
        } else {
            (self.most_connected_ids(max), "most-connected")
        };

        let files = ids.iter().map(|&id| self.context_file(id)).collect();
        let overview = self.overview();
        ContextPack {
            file_count: overview.file_count,
            languages: overview.languages,
            most_connected: overview.most_connected,
            files,
            selected_by: selected_by.to_string(),
        }
    }
}

impl Graph {
    /// File import degree (in + out), keyed by `FileId`.
    fn degrees(&self) -> HashMap<FileId, usize> {
        let mut deg: HashMap<FileId, usize> = HashMap::new();
        for &(a, b) in &self.imports {
            *deg.entry(a).or_insert(0) += 1;
            *deg.entry(b).or_insert(0) += 1;
        }
        deg
    }

    /// The `max` most import-connected files (degree desc, id as tiebreak).
    fn most_connected_ids(&self, max: usize) -> Vec<FileId> {
        let deg = self.degrees();
        let mut ids: Vec<FileId> = (0..self.files.len() as u32).map(FileId).collect();
        ids.sort_by(|&a, &b| {
            let (da, db) = (
                deg.get(&a).copied().unwrap_or(0),
                deg.get(&b).copied().unwrap_or(0),
            );
            db.cmp(&da).then(a.0.cmp(&b.0))
        });
        ids.truncate(max);
        ids
    }

    /// Seed files first, then their import neighbors (by degree) — the blast radius.
    fn context_by_seeds(&self, seeds: &[String], max: usize) -> Vec<FileId> {
        let adj = self.import_adjacency();
        let seed_ids: Vec<FileId> = seeds
            .iter()
            .filter_map(|s| self.file_id(Path::new(s)))
            .collect();
        let mut seen: HashSet<FileId> = HashSet::new();
        let mut ordered: Vec<FileId> = Vec::new();
        for &id in &seed_ids {
            if seen.insert(id) {
                ordered.push(id);
            }
        }
        let mut neighbors: Vec<FileId> = Vec::new();
        for &id in &seed_ids {
            for &nb in &adj[id.0 as usize] {
                if !seen.contains(&nb) && !neighbors.contains(&nb) {
                    neighbors.push(nb);
                }
            }
        }
        neighbors.sort_by(|&a, &b| {
            adj[b.0 as usize]
                .len()
                .cmp(&adj[a.0 as usize].len())
                .then(a.0.cmp(&b.0))
        });
        for id in neighbors {
            if ordered.len() >= max {
                break;
            }
            ordered.push(id);
        }
        ordered.truncate(max);
        ordered
    }

    /// Files ranked by query-term matches against path + symbol names (centrality tiebreak).
    fn context_by_query(&self, query: &str, max: usize) -> Vec<FileId> {
        let terms = tokenize(query);
        if terms.is_empty() {
            return Vec::new();
        }
        let deg = self.degrees();
        let mut syms_by_file: HashMap<FileId, Vec<String>> = HashMap::new();
        for s in &self.symbols {
            syms_by_file
                .entry(s.file)
                .or_default()
                .push(s.name.to_lowercase());
        }

        let mut scored: Vec<(f64, FileId)> = Vec::new();
        for f in &self.files {
            let path = f.path.to_string_lossy().to_lowercase();
            let mut score = 0.0;
            for t in &terms {
                // A path/name match is the strongest signal that this file IS the target.
                if path.contains(t) {
                    score += 5.0;
                }
                // Symbol-name matches help, but cap per term so a big file full of `*_order`
                // symbols can't outrank the conceptually-relevant file (no raw term-frequency).
                if let Some(syms) = syms_by_file.get(&f.id) {
                    let hits = syms.iter().filter(|n| n.contains(t)).count().min(3);
                    score += hits as f64;
                }
            }
            if score > 0.0 {
                score += deg.get(&f.id).copied().unwrap_or(0) as f64 * 0.1;
                scored.push((score, f.id));
            }
        }
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.1 .0.cmp(&b.1 .0))
        });
        scored.into_iter().take(max).map(|(_, id)| id).collect()
    }

    /// Build a [`ContextFile`] (symbols + deps/dependents) for one file.
    fn context_file(&self, id: FileId) -> ContextFile {
        let path = self
            .file_path(id)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let language = self
            .files
            .get(id.0 as usize)
            .map(|f| f.language.as_str().to_string());
        let mut symbols: Vec<String> = self
            .symbols
            .iter()
            .filter(|s| s.file == id)
            .map(|s| s.name.clone())
            .collect();
        symbols.truncate(15);

        let (mut depends_on, mut dependents) = (Vec::new(), Vec::new());
        if let Some(fd) = self.file_dependencies(&path) {
            depends_on = fd.dependencies;
            dependents = fd.dependents;
            dependents.truncate(25);
        }
        ContextFile {
            path,
            language,
            symbols,
            depends_on,
            dependents,
        }
    }
}

/// Distinct lowercased query terms of length ≥ 3 (split on non-alphanumerics).
fn tokenize(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() >= 3)
        .map(|w| w.to_lowercase())
        .filter(|w| seen.insert(w.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three cohesive clusters (auth / ui / db) bridged by one shared util file — the exact
    /// shape that makes label propagation collapse to a single community. Returns the graph
    /// plus the util's path so tests can assert it is detected as a hub.
    fn bridged_graph() -> Graph {
        let mut g = Graph::new();
        let rs = LanguageId::new("rust");
        let f = |g: &mut Graph, p: &str| g.add_file(PathBuf::from(p), rs.clone(), 0);

        let login = f(&mut g, "src/auth/login.rs"); // 0
        let token = f(&mut g, "src/auth/token.rs"); // 1
        let session = f(&mut g, "src/auth/session.rs"); // 2
        let page = f(&mut g, "src/ui/page.rs"); // 3
        let button = f(&mut g, "src/ui/button.rs"); // 4
        let util = f(&mut g, "src/util/log.rs"); // 5  (the bridge/hub)
        let conn = f(&mut g, "src/db/conn.rs"); // 6
        let query = f(&mut g, "src/db/query.rs"); // 7

        // auth triangle
        g.add_import(login, token);
        g.add_import(token, session);
        g.add_import(login, session);
        // ui
        g.add_import(page, button);
        // db
        g.add_import(conn, query);
        // util imported by one file from each cluster → bridges all three
        g.add_import(login, util);
        g.add_import(page, util);
        g.add_import(conn, util);
        g
    }

    fn group_of(view: &GraphView, path: &str) -> u32 {
        view.nodes
            .iter()
            .find(|n| n.kind == NodeKind::File && n.path == path)
            .unwrap_or_else(|| panic!("no file node for {path}"))
            .group
    }

    #[test]
    fn clusters_do_not_collapse_across_a_shared_bridge() {
        let g = bridged_graph();
        let view = g.graph_view(false);

        let auth = group_of(&view, "src/auth/login.rs");
        let ui = group_of(&view, "src/ui/page.rs");
        let db = group_of(&view, "src/db/conn.rs");

        // Each cluster is internally one community...
        assert_eq!(auth, group_of(&view, "src/auth/token.rs"));
        assert_eq!(auth, group_of(&view, "src/auth/session.rs"));
        assert_eq!(ui, group_of(&view, "src/ui/button.rs"));
        assert_eq!(db, group_of(&view, "src/db/query.rs"));

        // ...and the three clusters are NOT merged into one (the label-propagation failure).
        assert_ne!(auth, ui);
        assert_ne!(auth, db);
        assert_ne!(ui, db);
    }

    #[test]
    fn shared_bridge_file_is_flagged_as_a_hub() {
        let g = bridged_graph();
        let view = g.graph_view(false);
        let util = view
            .nodes
            .iter()
            .find(|n| n.path == "src/util/log.rs")
            .unwrap();
        assert!(
            util.is_hub,
            "the util bridging three clusters should be a hub"
        );

        // A leaf inside a single cluster is not a hub.
        let token = view
            .nodes
            .iter()
            .find(|n| n.path == "src/auth/token.rs")
            .unwrap();
        assert!(!token.is_hub);
    }

    #[test]
    fn clustering_is_deterministic() {
        let groups_a: Vec<u32> = bridged_graph()
            .graph_view(false)
            .nodes
            .iter()
            .map(|n| n.group)
            .collect();
        let groups_b: Vec<u32> = bridged_graph()
            .graph_view(false)
            .nodes
            .iter()
            .map(|n| n.group)
            .collect();
        assert_eq!(groups_a, groups_b, "colors must be stable across runs");
    }

    #[test]
    fn graph_view_files_only_has_a_node_per_file_and_import_edges() {
        let g = bridged_graph();
        let view = g.graph_view(false);
        assert_eq!(
            view.nodes
                .iter()
                .filter(|n| n.kind == NodeKind::File)
                .count(),
            8
        );
        assert!(view.nodes.iter().all(|n| n.kind == NodeKind::File));
        assert_eq!(view.edges.len(), 8);
        assert!(view.edges.iter().all(|e| e.kind == EdgeKind::Import));
    }

    #[test]
    fn graph_view_with_symbols_adds_symbol_nodes_and_edges() {
        let mut g = bridged_graph();
        let login = g.file_id(Path::new("src/auth/login.rs")).unwrap();
        let session = g.file_id(Path::new("src/auth/session.rs")).unwrap();
        let span = Span {
            start_byte: 0,
            end_byte: 1,
            start_row: 0,
            start_col: 0,
        };
        let caller = g.add_symbol("authenticate".into(), SymbolKind::Function, login, span);
        let callee = g.add_symbol("validate".into(), SymbolKind::Function, session, span);
        g.add_call(caller, callee);

        let view = g.graph_view(true);
        assert_eq!(
            view.nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Symbol)
                .count(),
            2
        );
        // A symbol inherits its defining file's community.
        let sym = view
            .nodes
            .iter()
            .find(|n| n.label == "authenticate")
            .unwrap();
        assert_eq!(sym.group, group_of(&view, "src/auth/login.rs"));
        assert!(view.edges.iter().any(|e| e.kind == EdgeKind::Defines));
        assert!(view.edges.iter().any(|e| e.kind == EdgeKind::Calls));
    }

    #[test]
    fn subgraph_returns_the_neighborhood_within_depth() {
        let g = bridged_graph();
        let sub = g.subgraph("src/auth/login.rs", 1).unwrap();
        assert_eq!(sub.depth, 1);
        assert_eq!(
            sub.files,
            vec![
                "src/auth/login.rs".to_string(),
                "src/auth/session.rs".to_string(),
                "src/auth/token.rs".to_string(),
                "src/util/log.rs".to_string(),
            ]
        );
        assert!(g.subgraph("does/not/exist.rs", 1).is_none());
    }

    #[test]
    fn shortest_path_connects_across_the_bridge() {
        let g = bridged_graph();
        let path = g
            .shortest_path("src/auth/login.rs", "src/db/query.rs")
            .unwrap();
        assert_eq!(path.first().unwrap(), "src/auth/login.rs");
        assert_eq!(path.last().unwrap(), "src/db/query.rs");
        // login -> util -> conn -> query
        assert_eq!(path.len(), 4);
        assert_eq!(path[1], "src/util/log.rs");

        // Same file → trivial path; unmapped → None.
        assert_eq!(
            g.shortest_path("src/ui/page.rs", "src/ui/page.rs").unwrap(),
            vec!["src/ui/page.rs".to_string()]
        );
        assert!(g.shortest_path("src/ui/page.rs", "nope.rs").is_none());
    }

    #[test]
    fn context_by_query_ranks_matching_files() {
        let g = bridged_graph();
        let pack = g.context(&ContextRequest {
            query: Some("auth".into()),
            seeds: vec![],
            max_files: 3,
        });
        assert_eq!(pack.selected_by, "query");
        assert_eq!(pack.file_count, 8);
        // Only the src/auth/* files match the term.
        assert!(pack.files.iter().all(|f| f.path.contains("auth")));
        assert!(pack.files.iter().any(|f| f.path == "src/auth/login.rs"));
    }

    #[test]
    fn context_by_seeds_returns_the_neighborhood() {
        let g = bridged_graph();
        let pack = g.context(&ContextRequest {
            query: None,
            seeds: vec!["src/auth/login.rs".into()],
            max_files: 5,
        });
        assert_eq!(pack.selected_by, "seeds");
        let paths: Vec<&str> = pack.files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"src/auth/login.rs")); // the seed itself
        assert!(paths.contains(&"src/util/log.rs")); // a 1-hop neighbor
                                                     // The seed's ContextFile carries its dependencies (login imports token/session/util).
        let seed = pack
            .files
            .iter()
            .find(|f| f.path == "src/auth/login.rs")
            .unwrap();
        assert!(seed.depends_on.contains(&"src/util/log.rs".to_string()));
    }

    #[test]
    fn context_defaults_to_most_connected() {
        let g = bridged_graph();
        let pack = g.context(&ContextRequest {
            query: None,
            seeds: vec![],
            max_files: 2,
        });
        assert_eq!(pack.selected_by, "most-connected");
        assert_eq!(pack.files.len(), 2);
        assert_eq!(pack.file_count, 8);
    }
}
