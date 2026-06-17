# ADR-0001: Implementation language — Rust

- **Status:** Accepted
- **Date:** 2026-06-17
- **Deciders:** Quinn (QuirijnVanDerZanden)

## Context

MapAI must satisfy two requirements that the product itself flags as decisive:

- **FR-2 / A2 (hard Must):** ship as **one cross-platform binary** that runs identically on
  Windows/Mac/Linux — the "radical simplicity" differentiator versus heavier tools.
- **Scale mandate:** stay fast on **100k+ file monorepos** (§4 of the requirements).

It also needs an MCP server, a tree-sitter integration, a file watcher, and — per the
North Star (FR-8 / H1) — a pluggable per-language extractor interface that outside
contributors can extend.

A key clarification framed the decision: **the implementation language is independent of
the languages MapAI can map.** tree-sitter is a C library with grammars for every target
language, so Python, Rust, Go, and Java can all parse all targets equally — target-language
**coverage is not a differentiator** and was not scored.

The user's initial preference was **Python**.

> Date-sensitive facts relied on (researched 2026-06-17, current sources):
> - Rust MCP SDK **rmcp v1.7.0** (2026-05-13), official, post-1.0.
> - tree-sitter Rust binding **0.26.x** (in-repo, releases in lockstep with the C core),
>   grammars statically linked at build time via the `cc` crate → no runtime C dependency.
> - `ignore` crate (ripgrep's gitignore-respecting parallel walker) + `rayon` for
>   GIL-free parallel parsing; `notify` (62.7M+ downloads) for cross-platform file watching.
> - `ast-grep` is a live proof point: Rust + tree-sitter + bundled grammars shipping a
>   single native binary at monorepo scale.
> - Python: no cross-compilation in PyInstaller/Nuitka and native tree-sitter wheels are
>   per-OS/arch → cannot produce one cross-platform binary (best fallback: `uvx mapai`);
>   GIL forces multiprocessing for parallel parsing.
> - Go: produces a single static binary, but its only production-grade tree-sitter binding
>   requires **CGO**, which breaks Go's clean cross-compile (fragile per-target C-toolchain
>   + Zig/GoReleaser pipeline); the pure-Go alternative is pre-1.0/single-maintainer.
> - Java: GraalVM native-image is immature with native deps; tree-sitter Java binding needs
>   JDK 23 (non-LTS) and ships grammar `.so` files; ~150–300 MB RAM baseline.

## Options considered

Scored on a weighted matrix (Distribution w5, Performance@scale w5, tree-sitter binding
w4, MCP SDK w3, file-watcher w3, contributor-friendliness w3). Full matrix and per-language
research are archived in the design discussion; totals below.

1. **Rust — total 105.** Wins both weight-5 criteria: true self-contained single binary
   (tree-sitter + grammars statically linked) and best-in-class GIL-free scale (`ignore` +
   `rayon`). Most self-contained tree-sitter build. *Cons:* steepest learning curve, a
   C/C++ toolchain needed to build grammars, real release-engineering (per-target CI matrix).
2. **Go — total 96.** Near-identical performance, single static binary, friendliest
   onboarding. *Con (decisive):* the only production-grade tree-sitter binding requires CGO,
   which destroys Go's clean cross-compile; the pure-Go escape hatch is pre-1.0/single-maintainer.
3. **Python — total 88 (user's preference).** Best MCP SDK, cleanest grammar story (305
   precompiled grammars, zero build step), easiest contributor onboarding. *Cons:* cannot
   produce a single cross-platform binary (forced to `uvx`/per-OS bundles); weakest at the
   100k-file scale (GIL → multiprocessing, heavy cold start).
4. **Java — total ~63.** Best-backed MCP SDK and true multithreading, but no clean
   single-binary path, the least mature tree-sitter binding, and a heavy runtime. Gives
   Python's distribution downsides without Python's ergonomic upsides. Eliminated.

When the single-binary Must was relaxed to "a clean one-line install is acceptable," the
matrix re-scored to Rust ≈ 92, Python ≈ 87, Go ≈ 85 — a near-tie between Rust and Python
whose remaining gap is driven entirely by performance-at-scale.

## Decision

**Implement MapAI in Rust.**

Even with the single-binary requirement relaxed, Rust remained the top-scored option, and
the user elected to take the highest-ceiling foundation: a genuine dependency-free single
binary, top-tier performance on large repos, and the most self-contained tree-sitter
integration — accepting a steeper learning curve as the cost.

## Rationale

- **It best satisfies the two decisive requirements (FR-2 and the scale mandate)** — the
  only candidate that delivers a true single cross-platform binary *and* GIL-free
  multi-core parsing, with a proven reference stack (`ignore` + `rayon` + tree-sitter, the
  `ast-grep` architecture).
- **The tree-sitter binding is the one non-recoverable separator** from Go: Rust's binding
  is the official in-repo, statically-linked one (no runtime C dependency); Go's forces
  CGO and Python can't make the binary at all.
- **The North Star (H1) maps cleanly onto Rust** — a `trait`-based extractor interface plus
  one feature-gated crate per language keeps each language a self-contained unit (see ADR-0002).

## Consequences

- **Positive:** the A2 promise is met with the strongest possible artifact (one
  dependency-free native binary); top-tier scale/memory on large monorepos; near-instant
  startup for a per-session MCP server; a stable post-1.0 official MCP SDK (rmcp); the
  extractor North Star expressed idiomatically as traits + per-grammar cargo features.
- **Negative / trade-offs accepted:**
  - **Contributor onboarding friction** (borrow checker + a C/C++ toolchain to build
    grammars) — the chief risk to the OSS North Star. *This is the cost we are explicitly
    choosing*, and it is actively mitigated (see Follow-ups).
  - **Release engineering:** building Windows/macOS(arm64+x64)/Linux(glibc+musl) is a
    per-target CI matrix because tree-sitter is C — standard, but real work.
  - **No official Rust grammar pack:** grammar crates are pinned/vendored per language;
    some (C++) need `build.rs` tuning.
  - **Going against the user's stated Python fluency** — accepted, and kept partly
    reversible by the language-agnostic core + stable extractor interface.
- **Follow-ups (onboarding mitigations — required, tracked in CONTRIBUTING.md):**
  1. A thin, well-documented `Extractor` trait (ADR-0002).
  2. A copy-paste **"add a new language" template** crate + step-by-step guide.
  3. **Fixtures-driven tests** per language (FR-15) so contributions are guided and safe.
  4. **Feature-gated grammars** so a contributor compiles only the language they touch.
  5. A ready-to-build dev environment (documented toolchain / dev container) so
     `cargo build` works on clone.
