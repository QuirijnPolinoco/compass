# ADR-0006: context pre-injection (the default delivery), MCP kept for deepening

- **Status:** Accepted
- **Date:** 2026-06-19
- **Deciders:** Quinn (QuirijnVanDerZanden)

## Context

Compass v1 delivered the map to AI agents one way: as **MCP tools the agent calls**
(`overview`, `file_dependencies`, `subgraph`, …). A controlled A/B benchmark
([`docs/benchmarks/`](../../benchmarks/README.md)) and an independent analysis both found
the same thing: for an already-capable agent, the **MCP tool-loop is roughly a wash** on cost.
Every tool call is an extra turn, every turn is extra tokens, and the agent often
double-checks the tool against grep — so the protocol overhead cancels the savings. A peer
tool (GrapeRoot) reached the same conclusion and pivoted away from the tool-loop.

The cost win comes from **not making the agent explore at all**: load the relevant slice of
the map into the prompt *before* the agent reasons. The open question is whether to replace
MCP with pre-injection or run both.

## Decision

**Add pre-injection as the default delivery; keep MCP as the deepening + portability layer
(hybrid).**

- New language-agnostic query `MapQuery::context(ContextRequest) -> ContextPack`: a
  **token-bounded** slice of the map — a short structural summary plus the top-N most relevant
  files, each with its defined symbols and its imports/dependents. Selection is:
  - **seed files given** (the files being worked on) → their 1-hop neighborhood (blast radius);
  - **else a query string** → files ranked by query-term matches against path + symbol names,
    with a centrality tiebreak;
  - **else** → the most-connected files (a structural primer).
- New CLI `compass context <repo> [--query TEXT] [--file PATH]… [--max N]` renders the pack as
  a compact markdown block (the CLI owns rendering; core stays presentation-agnostic, per
  ADR-0004).
- **Delivery into the agent is host-specific** and lives outside the binary: e.g. a Claude
  Code `UserPromptSubmit` hook that runs `compass context --query "<prompt>"` and injects the
  output. Compass ships the *producer* (`compass context`); each host wires the *injection*.
- **MCP is retained**, demoted from "the only way" to: (a) the **on-demand deepening** path
  when the agent needs something the pre-injected slice didn't cover, and (b) the
  **cross-host portability** story (the one universal protocol — FR-5/C2), since hooks are
  per-host.

## Rationale

Pre-injection removes the exploration turns the benchmark showed were the cost, so it should
be the default. But it is a **prediction** — on a large repo you can only inject a slice, and
when it misses, the agent must fetch on demand or it falls back to grep (the tax we're
removing). MCP is that escape hatch, and it is also the only delivery that works across
non-Claude hosts. Removing MCP would trade a core promise (works with any assistant) for
nothing — the cost concern is fully addressed by making pre-injection the default path so the
agent rarely *needs* a tool call for routine context. This matches the independent
recommendation: pre-inject a summary at session start, deepen with MCP during work.

## Consequences

- **Positive:** the common case is cheap (no tool-call turns); same graph powers the human map,
  the AI tools, and the injected context (one source of truth); MCP/portability intact.
- **Negative / trade-offs accepted:** pre-injection is heuristic (term-match + centrality; no
  recency/edit-weight yet — those need git/session data, noted as future); injection is wired
  per host (we ship the producer, not every host's hook); `context` ranking quality depends on
  the graph's completeness — which is why dependent-recall (the path-qualified-usage gap from
  the benchmark) should be fixed alongside.
- **Follow-ups (done 2026-06-19):** `context` on the query port + `Graph` impl + tests; the
  `compass context` CLI + `--hook`; the Claude Code `UserPromptSubmit` hook + `/compass`
  command; README; and the **session graph** — `compass context --hook` keys off the payload's
  `session_id` and skips files already injected this session (tracked in
  `.compass/sessions/<id>.json`), so a long session stops re-paying for context the agent
  already holds.
