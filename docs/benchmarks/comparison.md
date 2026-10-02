# Compass vs a competing graph tool vs plain search

> Question: which gets an AI agent to the right file fastest and cheapest, and which keeps its
> map fresh fastest after an edit? Every number below was measured on the machine noted. Where a
> result does not favour Compass, it is reported anyway.

The competitor is an open-source code knowledge-graph tool for AI assistants, written in
Python, that also builds its graph with tree-sitter. It was run in its code-only mode (no LLM)
at its latest release on 2026-10-01. It is not named here; the comparison is about approaches,
not about one project.

## TL;DR (final cycle)

- **Re-indexing after an edit:** Compass takes **0.12s** on a 613-file repo, the competitor
  **19.6s**. A cold build is 0.35s against 15.1s.
- **Agents:** Sonnet agents with Compass's injected map reached the right file and line with
  **33% fewer tool calls** than plain search and **42% fewer** than the competitor, and with
  the lowest median time per task (**9.8s**, against 12.5s and 14.5s). All three conditions
  answered 12/12 correctly.
- **Tokens:** Compass used the fewest (**1.3%** fewer than plain search, **2.4%** fewer than the
  competitor). The saving is small because a fixed ~40k-token session overhead (system prompt,
  tool definitions) dominates every run; the injected map itself is about 630 tokens.
- **Retrieval:** the correct file is **first for 8 of 12** tasks and in the top 5 for 10, against
  2 and 4 for the competitor's query command and 1 and 4 for a first `git grep`. On 12 untuned
  tasks in two new repos it is first for 5, and agents still need 17% fewer tool calls than
  plain search and 25% fewer than the competitor (see *Untuned validation*).

## Setup

- **Subjects:** this repo (Rust) and a private 613-file Rust + TypeScript trading app. Each tool
  got its own clone so neither indexed the other's cache.
- **Competitor:** built with its code-only extract command, re-indexed with its update command,
  queried with its query command. That condition got the exact instructions the tool's own
  Claude Code installer writes into `CLAUDE.md`, plus its CLI.
- **Compass:** release build. The prompt carried the output of `compass context --query <task>`
  the way the `UserPromptSubmit` hook injects it, plus the CLI for deepening.
- **Plain search (base):** Glob, Grep and Read only.
- **Tasks (12):** "where do I change X" prompts in plain language, each with one known correct
  file and symbol, for example "Raise the medium-risk profile's maximum drawdown cap from 15% to
  18%" or "Add bun.lockb to the files that are mapped but never analysed". Agents had to answer
  with the file, the symbol and line, and the change, without editing.
- **Machine:** Windows 11, 16 logical cores, NVMe SSD, Defender on.

## Three cycles

| Cycle | Compass build |
|---|---|
| 1 | `main` before #44: substring ranking, ~1,700-token slice |
| 2 | #44: word and rarity ranking, symbol lines, ~550-token slice |
| 3 (final) | #44 plus #45 to #49: fresh map per prompt, fields and enum variants, doc-comment summaries, map search, task text as a positional argument |

Plain search and the competitor did not change between cycles; they were rerun each time so all
three conditions share the same time window.

## Re-index speed (final cycle)

Median wall-clock, process start to exit (what a hook or a person waits for).

| | Compass (this repo) | Competitor (this repo) | Compass (613 files) | Competitor (613 files) |
|---|---|---|---|---|
| Cold build | 0.076s | 5.0s | 0.35s | 15.1s |
| Re-index, nothing changed | 0.033s | 5.0s | 0.10s | 20.1s |
| Re-index after 1 edited file | 0.093s | 5.1s | 0.12s | 19.6s |
| Re-index after 10 edited files | 0.095s | 6.0s | 0.13s | 19.5s |
| One query | 0.027s | 0.48s | 0.097s | 1.2s |

The competitor's update re-runs its whole cross-file build and clustering; its cache only skips
re-parsing (skipping clustering still took about 30s on the larger repo in cycle 1). Compass's
query now includes a check that the cached map still matches the working tree (#45), which is
why it went from about 0.02s to 0.1s on the larger repo.

## Retrieval (deterministic)

What each tool hands the agent for the task prompt, and where the correct file lands in it.
Tokens are characters / 4.

| | First | Top 5 | Found at all | Mean tokens |
|---|---|---|---|---|
| `git grep -il <keyword>` | 1/12 | 4/12 | 10/12 | 1,162 |
| Competitor's query | 2/12 | 4/12 | 6/12 | 1,505 |
| Compass, cycle 1 | 1/12 | 5/12 | 8/12 | 1,712 |
| Compass, cycle 2 | 6/12 | 10/12 | 10/12 | 553 |
| **Compass, final** | **8/12** | **10/12** | **11/12** | **626** |

The one task Compass still misses ("the panel where a user switches on real-money trading" for a
`LiveEnablePanel` component) is found at rank 8 once doc comments are read, but not in the top 5.
The competitor misses it as well.

## Agents (live A/B)

Twelve tasks, three conditions, Sonnet, one run per cell per cycle. Tool calls exclude the one
call every agent made to read its brief. API latency drifted between cycles, so compare within a
cycle.

| Cycle | | Total tokens | Tool calls | Median time per task | Total time | Correct |
|---|---|---|---|---|---|---|
| 1 | Plain search | 566,672 | 41 | 14.4s | 163.9s | 12/12 |
| 1 | Competitor | 551,724 | 46 | 15.9s | 201.5s | 12/12 |
| 1 | Compass | 581,597 | 37 | 11.0s | 139.8s | 12/12 |
| 2 | Plain search | 558,537 | 45 | 20.5s | 246.8s | 12/12 |
| 2 | Competitor | 552,075 | 47 | 21.1s | 265.1s | 12/12 |
| 2 | Compass | 549,396 | 31 | 14.8s | 172.7s | 12/12 |
| **3** | Plain search | 550,537 | 42 | 12.5s | 155.2s | 12/12 |
| **3** | Competitor | 556,636 | 48 | 14.5s | 223.1s | 12/12 |
| **3** | **Compass** | **543,188** | **28** | **9.8s** | 172.5s | 12/12 |

In cycle 1 Compass was already the fastest but spent the most tokens: its ~1,700-token slice cost
more than it saved. From cycle 2 on it spends the fewest. In cycle 3 one Compass run took 68s for
3 tool calls (API latency; its siblings took 8 to 11s), which is why its total time is higher
than plain search while its median is the lowest of the three; without that run its total is
104s.

## Untuned validation

Cycles 1 to 3 used tasks the ranking was tuned on. To see how much of the gain carries over,
the final build was frozen and run on 12 new tasks in two repos Compass had never been
benchmarked on: a ~200-file TypeScript chat bot and a Go networking project (~40 Go files, plus
F# that Compass does not map). Separate agents that were never shown any ranking code wrote
the tasks and picked the answers; at least half of them describe behaviour instead of naming
the file or symbol. Every answer was checked against the code before the run.

Agents, same three conditions and harness as above:

| | Plain search | Competitor | **Compass** |
|---|---|---|---|
| Correct | 12/12 | 12/12 | 12/12 |
| Total tokens | 545,966 | 541,896 | **536,775** |
| Tool calls | 36 | 40 | **30** |
| Median time per task | 11.9s | 13.4s | **10.2s** |
| Total time | 149.9s | 159.4s | **125.4s** |

Retrieval on the same tasks:

| | First | Top 5 | Found at all | Mean tokens |
|---|---|---|---|---|
| `git grep -il <keyword>` | 5/12 | 8/12 | 12/12 | 106 |
| Competitor's query | 3/12 | 4/12 | 6/12 | 1,553 |
| Compass | 5/12 | 7/12 | 8/12 | 353 |

The agent-level advantage holds on new code, at a smaller size: 17% fewer tool calls than plain
search (33% on the tuned tasks) and 25% fewer than the competitor, and the shortest times. The
ranking drops more, from first for 8 of 12 tuned tasks to 5 of 12; that gap is how much the
tuned figure was fitted to its tasks. The grep row is generous: its keyword was chosen by the
task writer while looking at the answer.

What the four misses have in common, and what Compass should index next:

- The value to change lives in a **string literal** (`'Maybe'`, `'twitter.com'`), which Compass
  does not see.
- It is a **key in a configuration object** (`qualityContent: { minEmojiCount }`), also not
  indexed.
- **Short stems**: "fixed" does not match `fix`.
- A **rare short word** in the prompt ("bot") matches one file name strongly and outranks the
  answer.
- The repo's doc comments are **in another language** than the prompt, so summaries do not help.

## What changed in Compass

- **#44, ranking:** identifiers and paths are split into words, lightly stemmed and matched by
  word or prefix instead of raw substring; filler is dropped; each term is weighted by how rare
  it is. Each listed file shows its matching symbols with their line (`medium_risk:120`), import
  lists are capped, and the default is 8 files.
- **#45, freshness:** the hook checks the cached map against the working tree on every prompt
  and reindexes only after an edit, so it never injects an outdated map.
- **#46, fields and enum variants** are symbols in 11 languages, and big files no longer win just
  by having many symbols.
- **#47, doc comments:** the comment above each symbol and each file's leading doc become one-line
  summaries used for ranking and shown in the pack.
- **#48, map search:** `compass map` searches by task with the same ranking.
- **#49:** `compass context "some task"` treats text that is not a path as the task.

## Limitations

- n = 1 per cell per cycle; compare within a cycle.
- 12 tasks per set, all "find where to change X". The ranking was tuned on the first set, so
  read its retrieval numbers next to the untuned validation above.
- The competitor ran code-only. Its LLM-backed extraction for docs and papers was not tested.
- Cycle 3 was measured on a local build of `main` with #45 to #49 applied, before they were
  merged.
