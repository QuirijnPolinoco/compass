# ADR-0005: visualization subsystem & local viz server

- **Status:** Accepted
- **Date:** 2026-06-18
- **Deciders:** Quinn (QuirijnVanDerZanden)

## Context

Compass already produces a language-agnostic graph and serves it as text (CLI) and JSON
(the `.compass/graph.json` cache and the MCP tools). Users want what Obsidian's Graph View
and Neo4j Bloom give them: an **interactive, force-directed, live-updating visual map** they
can read and explore — files as nodes, imports as edges, colored/clustered, that re-lays-out
as the code changes. The map must stay **useful to a human and to the AI** from one source of
truth (FR-3/B1, and the AI side via FR-11/C3 + FR-17/E1).

This forces three decisions that the existing ADRs don't cover:

1. **Where the visualization lives** without violating the North Star (adding a language must
   never touch core/engine/mcp — ADR-0002) or the layering (§4).
2. **How the browser is fed the map and kept live**, given the "one small binary, no network,
   code-never-leaves-the-machine" promises (FR-2/A2, FR-7/G1).
3. **How a network listener coexists** with the threat model's current claim that Compass
   "listens on stdio only (no socket)" (§7).

A complication: the visual is for the *human*; the AI does **not** read the picture — it reads
the same graph via cheap structured queries. So "for the AI" means new query operations
(neighborhood subgraph, shortest import path), not a second renderer.

## Options considered

**A. Where the renderer lives**
1. A **new `compass-viz` crate**, a language-agnostic consumer of the query port — peer to
   `compass-mcp`. Depends on `compass-core` only; the CLI composition root wires it up.
2. A module inside `compass-cli`. Less ceremony, but mixes a substantial protocol/asset
   surface into the binary entrypoint and can't be tested or evolved in isolation.
3. A module inside `compass-engine`. Wrong layer — viz is an output surface, not orchestration,
   and engine is meant to stay headless.

**B. How the browser gets the map + live updates**
1. **Static self-contained `map.html`** (data inlined), regenerated on each change. Simple and
   serverless, but a `file://` page can't re-fetch a sibling JSON (browser security), so "live"
   means a full regenerate-and-reload that flashes and discards zoom/pan/selection.
2. **Localhost HTTP server** that serves the app and **pushes deltas over Server-Sent Events
   (SSE)**, so the open page updates in place — smooth and stateful, like Obsidian. Costs a
   local TCP listener and a little more code.
3. **Export to an external tool** (Gephi/Bloom/Obsidian). Reuses mature viewers but defeats the
   zero-setup goal and ships no built-in map.

**C. Rendering library** (must vendor offline — no CDN, local-first)
1. **Cytoscape.js** — pure JS, **zero dependencies**, single vendorable `cytoscape.umd.min.js`,
   MIT, force-directed `cose` layout, rich styling + pan/zoom/click events. Canvas renderer
   handles low-thousands of nodes comfortably.
2. **Sigma.js + graphology** — WebGL, scales to tens of thousands of nodes, but a multi-package
   ESM ecosystem that is harder to bundle into one offline file.
3. **vis-network / force-graph** — single-bundle and good-looking, but less styling/algorithm
   control than Cytoscape for the files↔symbols toggle and Bloom-style node theming.

**D. Server crate** (Rust)
1. **`tiny_http`** — pure-Rust, ~120 KB, minimal deps, chunked transfer → SSE-capable, **no
   async runtime**. Matches "one small binary."
2. **`axum`/`hyper`/`tower`** — trivial SSE, but pulls a large async stack into a binary whose
   identity is minimalism (tokio is already present via `rmcp`, but hyper+tower+tower-http are
   not).

## Decision

- **A1 — new `compass-viz` crate**, a language-agnostic consumer of the `compass-core` query
  **port** (exactly like `compass-mcp`). It depends on `compass-core` only — **never** on the
  engine or any language crate. It owns its own render DTOs (Cytoscape element JSON), so
  `compass-core` stays render-unaware, consistent with ADR-0004's DTO-ownership rule.
- **B2 — localhost HTTP server with SSE push.** A new `compass map` command serves the
  interactive map on **`127.0.0.1` only** and streams `update` events over SSE off the existing
  `engine::watch` loop, so edits glide into the open page. `compass map --snapshot` additionally
  writes a **self-contained `.compass/map.html`** (app + data inlined) for an offline, serverless
  snapshot — so option B1 comes essentially for free as a side output.
  - **Port policy (must not collide with the user's other work or containers):** default to an
    **uncommon high port, `62049`**, chosen to sit well clear of the ports developers and
    containers actually use — typical dev servers (3000, 4200, 5173, 8000, 8080), databases
    (5432, 3306, 6379, 27017), and container/registry/Docker ports (5000, 2375/2376, 9090). If
    that port is already bound, **automatically retry on an OS-assigned free port** (`bind :0`)
    rather than failing. Compass always **prints and auto-opens the exact
    `http://127.0.0.1:<port>` URL**, so the actual number never matters to the user. `--port N`
    pins a specific port (e.g. for a stable bookmark); `--no-open` suppresses the browser launch.
    A collision with another service is therefore impossible by construction.
- **C1 — Cytoscape.js**, vendored as a single `cytoscape.umd.min.js` under
  `crates/compass-viz/assets/` and embedded into the binary via `include_str!`. Fully offline,
  MIT-compatible with the dual MIT/Apache repo.
- **D1 — `tiny_http`.** Synchronous, thread-per-connection; SSE via chunked transfer. No new
  async runtime in the viz path.
- **Query additions land in `compass-core`, not in any language crate.** The `MapQuery` port
  gains `graph_view` (all nodes/edges for rendering, files-only or files+symbols), `subgraph`
  (neighborhood around a file — FR-11/C3, FR-18/E2), and `shortest_path` (import path between two
  files — FR-17/E1). These are language-agnostic graph queries; core owns the query engine (§4),
  so adding them touches core + its `Graph` impl and **no** `lang-*` crate. `compass-mcp` exposes
  `subgraph`/`shortest_path` as new tools; `compass-viz` consumes `graph_view`/`subgraph`.
- **Node grouping is by detected community (graph structure), not by folder.** Nodes are
  colored/clustered by **structural community detection** over the file import graph — files that
  depend densely on each other form a group (a "sub-part of the project"), which is what a human
  wants to see. This is chosen *because* repo conventions vary: it works whether the project is
  organized **by feature** (groups ≈ folders) or **by type** (a controller→service→model feature
  scattered across `controllers/`, `services/`, `models/` still groups together, because grouping
  reads *edges, not paths*). The pass runs **server-side in `compass-core`, deterministically**
  (label propagation with fixed node ordering — no RNG — so colors are stable across reloads); a
  `graph_view` node carries a `group` id and an `is_hub` flag. **Hubs** (files imported across many
  groups, e.g. shared utils) belong to no single group and render **neutral/gray** (matching the
  gray central nodes in Obsidian's graph). The renderer offers **by-community (default)**, plus
  **by-folder** and **by-language** as one-click alternate color modes (the data is already
  present). *Limitation:* community detection is heuristic; on a degenerate graph it may
  over-merge — acceptable for a visual aid, and the force layout still separates clusters
  visually. Upgrade path: swap label propagation for Louvain/Leiden behind the same `group` field.

### Threat-model exception (the deliberate part)

§7 states the MCP server "listens on stdio only (no socket)." That remains true — **the MCP
server is unchanged.** The viz server is a **separate, opt-in, localhost-only** listener that
exists *only* while `compass map` runs. It is admitted as a scoped exception with these
constraints, which are part of the decision (not afterthoughts):

- **Bind `127.0.0.1` only**, never `0.0.0.0`; there is intentionally **no flag** to expose it
  on a public interface.
- **Read-only**: all endpoints are `GET`; the server never mutates the repo, never executes
  mapped code, and writes nothing (the one file write, `--snapshot`, is an explicit, separate
  user action, not an endpoint).
- **Ephemeral**: the listener dies with the command; it is not started by `serve`/`overview`/
  `init` and is never on by default.
- **Serves only the already-mapped graph** (the same data already in `.compass/`).

Residual risk (right-sized, local read-only tool): other local processes or browser tabs on the
same machine could reach the port (a CSRF/local-multi-user concern). Accepted for v1 given the
data is non-secret local map data and the surface is read-only. **Future hardening (noted, not
v1):** a random ephemeral port plus a per-session token in the served URL, and an `Origin`/`Host`
header check.

## Rationale

A separate crate keeps the durable layering intact (`compass-viz` is to the browser what
`compass-mcp` is to the AI host) and keeps the North Star untouched — neither the visual nor the
new queries require editing any language crate, and no language crate is needed to change the
visual. SSE over a localhost socket is the only option that delivers the *stateful, in-place*
live updates the request is really about; a static file cannot, and an external tool abandons
zero-config. Cytoscape.js + tiny_http are the choices that preserve "one small offline binary":
both are dependency-light, vendorable, and need no CDN or async runtime. Putting the new queries
in core (not a new "AI" surface) keeps one source of truth — the same `subgraph` powers the UI's
focus mode and the AI's token-cheap fetch.

## Consequences

- **Positive:** a human-readable visual map and token-cheap AI queries, both off one graph;
  the visual is live without a page flash; the binary stays offline and self-contained; the
  language seam and core/mcp layering are untouched (verifiable by the compiler — `compass-viz`
  can only reach `compass-core`).
- **Negative / trade-offs accepted:** a new (localhost, opt-in, read-only) network surface and
  its documented residual risk; a vendored third-party JS asset to keep current; `MapQuery` grows
  (a coordinated change to core + its `Graph` impl, but to no language crate); Cytoscape's canvas
  renderer sets a soft node-count ceiling (mitigated by files-only default, folder clustering,
  and lazy symbol expansion — Sigma/WebGL is the escape hatch if 100k-node repos demand it).
- **Follow-ups:** add `compass-viz` to the workspace; add the `MapQuery` methods + `Graph` impl
  + tests; add the `subgraph`/`shortest_path` MCP tools; add `compass map` to the CLI; vendor
  Cytoscape.js with its MIT notice; update §1/§3/§4/§6/§7/§8/§9 of the architecture doc and the
  container + a new viz-flow diagram; document `compass map` in the README and CONTRIBUTING.
