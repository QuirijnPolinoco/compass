# Requirements — Compass

> Status: **CONFIRMED** by Quinn at the design-review gate (2026-06-17), after an
> independent architecture review.
> The rest of the architecture is measured against this document.
> Source of product vision: [`ProjectInfo.md`](../../ProjectInfo.md). This file is the
> formalized, testable restatement of that vision.

## 1. Purpose

- **Problem:** AI coding assistants waste tokens grepping a whole repo to find the right
  files, and sometimes edit the wrong ones or invent paths that don't exist. Humans new
  to a codebase face the same "where does anything live and how does it connect" problem.
- **Goal / definition of success:** one command, run inside any repo, produces an
  accurate, queryable map (what files exist, how they connect via imports/calls, where
  the important logic lives) — usable directly by a human and served to any AI assistant
  over MCP. Code never leaves the machine.

## 2. Users & roles

| Role | Who they are | What they need to do |
|------|--------------|----------------------|
| AI-assistant user | Dev who wants their assistant to navigate accurately | Run Compass in a repo; point their MCP-capable assistant at it |
| Human explorer | New joiner, returning solo dev, code reviewer | Read the map to understand structure and dependencies — with or without AI |
| Language contributor | OSS contributor adding a language | Implement one extractor against a stable interface, ship it without touching core |

Approximate user count: open-source, unbounded. External: yes (public project).

## 3. Functional requirements

IDs map to the epics/stories in `ProjectInfo.md`. Priority is MoSCoW from the spec.

### MVP (Must — no release without these)
- **FR-1 (A1):** Install and run with **one command, zero config**; map a repo in minutes.
- **FR-2 (A2):** Ship as **one cross-platform binary** (Windows/Mac/Linux), identical results, no environment setup. *(See ADR-0001 — Rust delivers a true static binary; a one-line installer was an accepted fallback but is not needed.)*
- **FR-3 (B1):** Produce a **clear human-readable overview** of structure and how files connect.
- **FR-4 (C1):** Serve the same map over an **MCP server** so an AI finds the right files without grepping.
- **FR-5 (C2):** Work with **any major model** (Claude, Gemini, ChatGPT, Grok, DeepSeek, Llama) — achieved by speaking MCP and never integrating a model directly.
- **FR-6 (D1):** The map contains **only files that actually exist**; the AI is steered to real, mapped paths (no invented paths).
- **FR-7 (G1):** Mapping runs **fully locally, no API key**; proprietary code never leaves the machine.
- **FR-8 (H1):** A **stable extractor interface** is in place; **Tier 1 languages** (Go, Python, Java, C#, TypeScript/JavaScript) are supported through it.

### Should (next, important)
- **FR-9 (A3):** Auto-respect **`.gitignore`** so build artifacts/dependencies aren't mapped.
- **FR-10 (B2):** Show a file's **dependencies and dependents** ("what I'll affect before I change it").
- **FR-11 (C3):** Let the AI fetch only the **relevant subgraph** for a task (small, cheap context). *(Delivered by the `subgraph` query/MCP tool, ADR-0005.)*
- **FR-12 (D2):** **Flag broken imports / references** to missing files.
- **FR-13 (F1):** **Live freshness** — update the map in real time as code is edited.
- **FR-14 (H2):** A **single source-of-truth list** of supported languages the tool reports.
- **FR-15 (H3):** **Per-language test fixtures** so a change to one language can't silently break another.
- **FR-20 (B — visual map):** Provide an **interactive visual map** — a force-directed graph of files (edges = imports), exploreable in the browser (pan/zoom/search/click), colored by language or folder, with an optional toggle to expand into symbols (defines/calls). The human-readable counterpart to the AI's MCP view. *(Added 2026-06-18, ADR-0005.)*
- **FR-21 (B — visual map, F1):** The visual map **updates live** as code is edited — the open page re-lays-out in place (no manual refresh), riding the same watcher as FR-13. *(Added 2026-06-18, ADR-0005.)*

### Could (nice-to-have)
- **FR-16 (B3):** Surface the **most-connected files** (where the important logic lives).
- **FR-17 (E1):** Answer **"what connects X to Y"** (path between two parts). *(Delivered by the `shortest_path` query/MCP tool, ADR-0005.)*
- **FR-18 (E2):** Answer **"what breaks if I change this"** (impact analysis).
- **FR-19 (H4):** Grow coverage over releases — Tier 2/3, then HTML/CSS reference edges.

### Explicitly out of scope (this release)
- SQL / database-schema extractor (separate graph model — later roadmap).
- Cloud sync, hosted dashboards, telemetry.

## 4. Scale & growth

- **Repo size:** design to stay fast on **large monorepos (100k+ files)**, even though most
  users will have smaller repos. Do not assume small repos; do not re-architect later.
- **Workload shape:** one expensive **full index** on first run, then **incremental**
  single-file reparses driven by the file watcher (steady state is cheap).
- **Growth axis:** number of **supported languages** (the North Star). The architecture's
  job is to make language #16 as easy to add as language #6.
- Local-only: no network traffic, no multi-tenant concerns, no geographic distribution.

## 5. Non-functional requirements (the ones that matter, with targets)

| Attribute | Target / requirement | Why it matters |
|-----------|----------------------|----------------|
| Performance (cold) | Full index of a large repo should be parallel and bounded by I/O + parse, not by language overhead; target "minutes, not coffee-break" on 100k files | A1 promise of "a map in minutes"; scale mandate |
| Performance (warm) | Single-file reparse on save in well under a second | F1 live freshness must not lag editing |
| Startup | Near-instant process start (no runtime warmup) | MCP host may launch the server per session |
| Privacy | 100% local; **no network calls**, no telemetry, no API keys; never executes mapped code | G1 — proprietary code stays on the machine |
| Correctness | A malformed/unparseable file must **never crash the index** — collect the error, keep going; map contains only real files | D1/D2 + robustness on messy real-world repos |
| Maintainability / extensibility | Adding a language touches **only** its own crate + one composition point; **zero** edits to core graph or MCP | H1 North Star; this is the primary design driver |
| Portability | One binary, identical behavior on Windows/macOS/Linux (x64 + arm64) | A2 |

## 6. Constraints

- **Budget:** none (no paid infra/services; local tool).
- **Timeline:** none hard; correctness and a clean foundation over speed of delivery.
- **Team:** primarily solo (Quinn), open to outside OSS contributors. Quinn's strongest
  language is Python; Rust was chosen on merits (ADR-0001) with an explicit plan to keep
  contributor onboarding low-friction.
- **Existing infrastructure:** none — greenfield. GitHub repo, will be open-sourced.
- **Integrations:** **MCP** (Model Context Protocol) as the AI interface; **tree-sitter**
  (C library + grammars) as the parsing engine. No model-specific integration.
- **Licensing / lock-in:** open-source license to be chosen (see Assumptions); prefer
  permissive, ecosystem-standard.

## 7. Assumptions to confirm

- **A-1:** Single-binary distribution via Rust is preferred over a one-line installer; a
  package-manager story (Homebrew/Scoop/`cargo-binstall`) is "nice to have," not required
  for v1. ✅ *confirmed 2026-06-17.*
- **A-2:** License is **dual MIT OR Apache-2.0** (the Rust ecosystem norm). ✅ *confirmed 2026-06-17.*
- **A-3:** v1 transport is **MCP over stdio** (local subprocess launched by the host);
  HTTP transport is deferred. ✅ *confirmed 2026-06-17.*
- **A-4:** "Human-readable overview" (FR-3/B1) is delivered initially as **CLI output**
  (e.g. a tree/summary command), not a GUI. ✅ *confirmed 2026-06-17.* **Extended 2026-06-18
  (ADR-0005):** a richer **interactive browser visualization** (`compass map`, FR-20/FR-21) is
  added alongside the CLI output — it does not replace it. The CLI text overview stays the
  zero-dependency default; the visual map is opt-in.
- **A-5:** The map is held **in memory** with an on-disk cache under `.compass/` for fast
  restart; not a database. ✅ *confirmed 2026-06-17.*
- **A-6 (ADR-0005):** The visual map is served by a **localhost-only** HTTP server bound to
  `127.0.0.1`, opt-in (only while `compass map` runs), read-only, on an uncommon high port with
  free-port fallback so it cannot clash with the user's services/containers. This is the single
  network listener and a deliberate, scoped exception to "stdio-only" (A-3, which is unchanged
  for MCP). Local-first/no-internet is preserved: the renderer is vendored and embedded, so the
  map works offline. *Proposed 2026-06-18 — to confirm at the design-review gate.*
