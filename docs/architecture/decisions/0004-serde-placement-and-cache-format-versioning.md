# ADR-0004: serde placement & cache-format versioning

- **Status:** Accepted
- **Date:** 2026-06-17 (from the independent architecture review)
- **Deciders:** Quinn (QuirijnVanDerZanden)

## Context

Two components must serialize the graph: `compass-engine::cache` writes it to `.compass/` for
fast restart, and `compass-mcp` serializes query results to JSON for the AI host. Both act on
the same `compass-core` domain types (`File`, `Symbol`, edges). The architecture says
`compass-core` is a pure domain model and `compass-mcp` owns the MCP wire schema — so we must
decide **where the `serde` derives live** without contradicting either claim, and what the
on-disk format's stability guarantee is.

## Options considered

1. **`serde` derives directly on `compass-core` types.** Pragmatic and idiomatic; one
   definition. *But* it makes core's serialized form a compatibility surface, and risks
   leaking MCP wire-shape concerns into core if we're not careful.
2. **Separate DTO structs everywhere; keep core serde-free.** Maximally decoupled, but
   forces hand-written/duplicated mappings for the cache *and* the MCP layer — drift-prone
   for no real benefit on the cache side.

## Decision

- **Put `serde` derives on `compass-core` domain types** (Option 1) and treat the core
  serialized form as a **versioned compatibility surface**: the `.compass/` cache carries a
  **cache-format version tag**. On load, a mismatched (or absent) version triggers a **clean
  reindex** — consistent with the content-hash "never trust the cache blindly" rule (§5).
- **Keep MCP wire/result types as thin `schemars`-deriving DTO wrappers in `compass-mcp`**, so
  the MCP tool schema is owned by `compass-mcp` and `compass-core` stays MCP-unaware. The cache
  uses core's `serde` form directly; the MCP layer maps core → its DTOs.

## Rationale

Deriving `serde` on core is the low-friction Rust choice and avoids duplicated cache
mapping; the only cost — that the on-disk form becomes a compatibility surface — is handled
cheaply and safely by a version tag plus reindex-on-mismatch (the cache is a rebuildable
optimization, never a source of truth). Isolating the *MCP* schema in DTOs preserves the
boundary the architecture promises (`compass-core` doesn't know about MCP) and lets the wire
schema evolve independently of the domain model.

## Consequences

- **Positive:** one definition of the domain types; trivial, safe cache invalidation; the
  MCP schema and the domain model evolve independently.
- **Negative / trade-offs accepted:** core's serialized shape is now a surface we must bump
  deliberately; a thin DTO mapping layer in `compass-mcp` to maintain.
- **Follow-ups:** define the cache-format version constant and the reindex-on-mismatch path
  in `compass-engine::cache`; document the bump rule (any breaking change to a serialized core
  type ⇒ bump ⇒ clean reindex).
