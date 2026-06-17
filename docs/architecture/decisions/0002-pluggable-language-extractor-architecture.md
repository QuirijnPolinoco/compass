# ADR-0002: Pluggable language-extractor architecture

- **Status:** Accepted
- **Date:** 2026-06-17 (rationale revised 2026-06-17 after independent architecture review)
- **Deciders:** Quinn (QuirijnVanDerZanden)

## Context

The North Star (FR-8 / H1) requires that **adding a language is a self-contained unit of
work** that does not touch the core graph, the MCP layer, or any other language. The
Definition of Done (`ProjectInfo.md` §4) makes "**No core changes**" an acceptance
criterion. We need to choose how, concretely, languages plug into the system in Rust — and
how the rest of the code stays language-agnostic.

A complication specific to a **single static binary**: there is no runtime plugin discovery
(we can't load `.so` plugins — the whole point is one self-contained binary). So "plugins"
must be **compiled in**, and the question is how to add one with the least possible coupling
to shared code. *How* registration happens is split into its own decision — see
[ADR-0003](0003-extractor-registration-explicit-registry.md).

## Options considered

1. **One big crate, a `match` on language in core.** Every new language edits core files;
   violates "no core changes" outright; a merge-conflict magnet. Rejected.
2. **One `mapai-languages` crate with a module + cargo feature per language.** Per-language
   code is isolated in modules behind features. Simple, few crates. *But* all languages
   share one `Cargo.toml` and one crate boundary, so they cannot be tested/built/version-
   pinned in true isolation.
3. **A Cargo workspace with one crate per language**, each depending only on the stable
   `mapai-extract` interface (+ `mapai-core` types), behind cargo features. Rejected
   "one crate" only at the cost of more crates.

## Decision

Adopt **Option 3**: a **Cargo workspace** with a stable interface crate and **one crate per
language**, behind cargo features.

- `mapai-extract` defines the **stable `Extractor` trait** and the supporting contract
  types. The trait has **two phases** (so per-language import resolution never leaks into
  the engine):
  - `extract(file, tree) -> { symbols, raw_imports }` — pure, per-file.
  - `resolve(raw_imports, &ResolutionContext, &LangConfig) -> resolved | unresolved` —
    whole-repo, but the **algorithm is the language crate's**; the engine only supplies a
    language-agnostic `ResolutionContext` (a read-only path→FileId view over all files plus
    the importing file's location) and an opaque per-language config carrier.
- Each `mapai-lang-<name>` crate implements `Extractor` for one language: it declares its
  `Detection { extensions, shebangs }`, owns its tree-sitter grammar dependency and queries,
  extracts symbols/imports, and resolves that language's imports. It depends on
  `mapai-extract` and `mapai-core` types **only** — never on another language crate.
- Each crate **ships its own fixtures + snapshot tests** (`crates/mapai-lang-<name>/tests/`)
  so `cargo test -p mapai-lang-<name>` is a self-contained unit that compiles exactly one
  grammar.
- The `Extractor` trait emits the **same node/edge shape for every language** (DoD §4), so
  nothing downstream special-cases a language.

## Rationale

Option 3 is the only design where the **compiler mechanically** forbids language→language
coupling and core edits — the property the North Star most needs. Its durable advantages
over Option 2 (a single feature-gated `languages` crate) are what justify the extra crates:

- **Independent grammar-version pinning** at the crate boundary — one language's grammar
  bump is isolated to its own `Cargo.toml` and can't perturb another.
- **Truly isolated `cargo test -p mapai-lang-<x>` and per-crate CI** — compile and test one
  grammar without the others; this is also how comparable Rust tools (ast-grep, biome)
  manage large grammar sets.
- **Per-crate incremental-compile granularity** — touching one language recompiles one
  crate, directly serving the project's biggest named risk (onboarding/compile friction).

*(Note: the earlier draft rejected Option 2 partly on "co-mingled grammar deps" and "a
shared registry list edit." Those are weak: `dep:`-gated optional deps mean a non-enabled
grammar in a shared `Cargo.toml` isn't compiled, and the registry-edit concern is moot under
the explicit-registry approach of ADR-0003. The durable per-crate-isolation reasons above
are the real discriminators, and are what a future contributor tempted to collapse the
crates should weigh.)*

## Consequences

- **Positive:** strongest possible isolation (the type system prevents cross-language and
  core coupling); per-language grammar pinning so one grammar bump can't break another;
  fast, focused builds/tests via per-crate features; the folder layout reads exactly as "one
  folder per language."
- **Negative / trade-offs accepted:** more crates to manage; one shared composition-root
  edit per language (the CLI feature + one `register()` line — see ADR-0003), which is **not**
  a core/MCP change.
- **Follow-ups:** ship a `mapai-lang-template` skeleton (extracted from the first real
  extractor, **excluded** from the workspace so it never links or gates CI); document the
  exact steps in CONTRIBUTING.md (the DoD checklist); add the two-job CI from §9 so an
  isolated grammar can't silently break the build and a language can't silently fail to
  register.
