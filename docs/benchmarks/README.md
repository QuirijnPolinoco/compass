# Does the map actually help an AI? — a Compass benchmark

> Subject: a private, mid-size **Rust + TypeScript Cargo workspace** (~280 files; all crate
> and file names are anonymized below). Honest by design — every number is measured or
> observed; nothing is invented, and findings are reported even where they don't flatter
> Compass.
>
> Looking for **indexing speed at scale** (does it handle 100k files)? See
> **[scaling.md](scaling.md)**.

## TL;DR

On a **controlled A/B** (one strong coding agent — Claude — given 10 tasks, identical in every
way except whether it could also use Compass), and a third **pre-injection** condition:

- **Tool calls to locate the answer:** grep-only **66** · Compass-as-an-MCP-tool **65** ·
  **pre-injected 45 (~31% fewer)**.
- **The MCP tool-loop alone was a wash** — calling a graph tool adds turns that cancel the
  savings. **Pre-injecting** the relevant map slice into the prompt is where the win is.
- **Navigation correctness was ~tie**, except the one query grep *can't express* — "which files
  are most central?" — where grep got 2/3 and Compass got 3/3.
- **Compass under-counts dependents** (a foundational crate: 39 found via `use` edges, but 44
  files truly use it — it misses fully-qualified path references with no `use`). Fix that and
  impact queries become complete.

**Verdict:** for an already-capable agent, Compass-as-a-tool-the-agent-calls is *not* a general
token-slasher over grep. Its value shows up two ways: (1) **structure** queries grep can't do
(centrality, the visual map, blast-radius), and (2) **pre-injection** — front-loading the map
slice cut effort ~31%. Deliver by pre-injection (ADR-0006), keep MCP for deepening.

## Setup

- **Conditions:** `grep` (no Compass; Read/Grep/Glob only) · `tool-loop` (same agent, may call
  the Compass CLI as it explores) · `pre-injected` (the `compass context` slice is prepended to
  the prompt — what the host hook injects — then the agent solves).
- **Controls:** identical prompt, identical tools, hard-pinned to the subject repo; the *only*
  difference between grep and tool-loop is whether Compass is offered. "With Compass" uses the
  Compass **CLI** as a measurable stand-in for the MCP server (same graph).
- **Scoring:** navigation tasks by **recall** vs ground truth (computed in-script); edit tasks
  by a separate **verifier agent** judging the proposed change against the real code. Effort =
  **self-reported tool calls to locate** the answer.
- **Tasks (10):** 5 navigation/impact + 5 edit, described by role (names anonymized):

  | ID | Task |
  |----|------|
  | N1 | List all files that directly depend on a **foundational crate** (the most depended-on) |
  | N2 | List all files that directly depend on a **second widely-used crate** |
  | N3 | List the internal module files of a **multi-module crate** |
  | N4 | Name the **3 most-connected/central** files in the repo |
  | N5 | Which **executables** are impacted by a breaking change to the second crate? |
  | E1 | Add a method to a **value type** |
  | E2 | Register a new item in a **registry** (find the registry + its trait) |
  | E3 | Add a variant to an **error enum** |
  | E4 | Add a hard cap/guard in a **policy function** |
  | E5 | Add a config flag to an **engine struct** |

## Results — live A/B (3 conditions)

`rec` = recall vs ground truth; `calls` = tool calls to locate (the pre-injected slice is free, 0).

| Task | grep | tool-loop | **pre-injected** |
|------|------|-----------|------------------|
| N1 dependents of crate A | rec 100% · 8 | rec 100% · 11 | rec 100% · **4** |
| N2 dependents of crate B | rec 100% · 7 | rec 100% · 19 | rec 100% · 7 |
| N3 modules of a crate | rec 100% · 2 | rec 100% · 3 | rec 100% · 3 |
| N4 most-central files | rec **67%** · 8 | rec 100% · 2 | rec 100% · 6 |
| N5 executables impacted | rec 100% · 8 | rec 100% · 5 | rec 100% · 7 |
| E1 method on value type | file✓ chg✓ · 9 | file✓ chg✓ · 6 | file✓ chg✗ · **4** |
| E2 register in registry | file✓ chg✓ · 5 | file✓ chg✓ · 6 | file✗ chg✗ · 7 |
| E3 error variant | file✓ chg✓ · 6 | file✓ chg✓ · 3 | file✓ chg✓ · **1** |
| E4 cap in policy fn | file✓ chg✓ · 6 | file✓ chg✗ · 4 | file✓ chg✓ · **2** |
| E5 flag on engine struct | file✓ chg✗ · 7 | file✓ chg✗ · 6 | file✓ chg✓ · **4** |
| **TOTAL** | **66** | **65** | **45** |

(edit cells: file = correct file, chg = correct change.) Edit *file* located: 5/5 · 5/5 · 4/5;
correct *change*: 4/5 · 3/5 · 3/5.

**Reading it:** the tool-loop matched grep on effort (65 vs 66) — protocol overhead eats the
savings. Pre-injection cut effort to **45 (~31% fewer)** while holding navigation recall and even
fixing some edits (E4, E5) the tool-loop got wrong. The honest losses: pre-inject got **E1**
(right file, wrong change) and **E2** (wrong file *and* change) — it leaned on an incomplete
injected slice and under-explored. On exhaustive lists (N1/N2) it still grepped, because the
context pack caps its dependent list — but fewer times than the alternatives (N1: 4 vs 8/11).

## Results — deterministic token cost (a bound, with a big caveat)

Tokens ≈ chars/4; reproducible. For "what depends on crate X" the Compass output vs. raw grep:

| Query | Compass output | `grep` output | `grep` + read every matched file |
|-------|---------------|---------------|-----------------------------------|
| dependents of crate A | **~431 tok** | ~4,256 tok (10×) | ~455,604 tok (~1,057×) |
| dependents of crate B | **~329 tok** | ~2,297 tok (7×) | ~377,139 tok (~1,146×) |
| a crate's own modules | ~447 tok | *read one file* ≈ **125 tok** ← grep wins | — |

**The caveat:** the "~1,000×" assumes the agent reads every grepped file. It doesn't — in the
live A/B the grep agent answered in ~8 tool calls using `grep -l` + targeted reads. So the
realistic per-query gap is the middle column (~7–10×), and even that didn't reduce *total* effort
at the task level (66 ≈ 65). "Read everything" is a strawman bound, shown only for transparency.

## Findings

1. **Pre-injection is the delivery that pays off** (~31% fewer calls); the MCP tool-loop is a
   wash for a capable agent. Hence: pre-inject by default, keep MCP for deepening (ADR-0006).
2. **Structure queries are Compass's other clear edge** — centrality / "most-connected" is
   something grep cannot express; Compass was both more correct and far fewer steps there.
3. **Compass under-reports dependents** — it tracks `use`/`mod`/`extern crate` edges, but misses
   inline fully-qualified path references (no `use`). It found 39 of the true 44 for one crate
   (high precision, ~89% recall here). Fixing this makes impact queries — and the injected
   context — complete.
4. **Faster can mean under-exploring.** Pre-injection's edit dips came from over-trusting an
   incomplete slice. Inject to *orient*; still verify/deepen before editing (the hybrid).
5. **For symbol-greppable lookups, a strong agent + grep is already competitive.** Compass's
   per-query output is smaller, but that alone didn't cut total effort in the tool-loop.

## Limitations (read before quoting any number)

- **n = 1** per cell — tool-call counts are noisy; trust the aggregate direction, not single rows.
- Tool calls are **self-reported**, not instrumented.
- The baseline agent is **strong** (Claude); Compass's relative edge would likely be larger
  against a weaker agent or a much larger repo where grep output explodes. This is a hard test.
- "With Compass" / "pre-injected" use the **CLI as a stand-in** for the MCP server.
- **Single repo**, Rust-heavy.

## What this means for Compass

- **Deliver by pre-injection, not a tool-loop** — the ~31% win, now the default (ADR-0006); MCP
  stays for deepening + cross-host portability.
- **Inject to orient, then deepen** — don't let the agent treat an incomplete slice as complete.
- **Lead with structure** (map / centrality / blast-radius) — the part grep genuinely can't do.
- **Fix dependent recall** (Finding 3): also resolve fully-qualified path usages.

## Reproduce

- **Live A/B:** a workflow runs each task under all conditions with identical prompts/tools;
  ground truth is produced by `compass deps`/`overview` on the subject repo and recall is scored
  in-script. **Pre-injection:** each agent's prompt is prepended with
  `compass context <repo> --query "<task>"` (the hook's output), then it solves.
- **Deterministic tokens:** for a crate `C` with import name `I`,
  `compass deps <repo> <C lib root> | wc -c` vs `grep -rnI "I" <repo> | wc -c` vs the summed size
  of the `grep -rlI "I"` matched files. Tokens ≈ chars / 4.
- Caveats above apply to every number (n=1, self-reported, CLI-as-MCP-proxy, strong agent).
