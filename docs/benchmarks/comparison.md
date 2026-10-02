# Compass vs a competing graph tool vs plain search

> Question: which gets an AI agent to the right file fastest and cheapest, and which keeps its
> map fresh fastest after an edit? Every number below was measured on the machine noted. Where a
> result does not favour Compass, it is reported anyway.

The competitor is an open-source code knowledge-graph tool for AI assistants, written in
Python, that also builds its graph with tree-sitter. It was run in its code-only mode (no LLM)
at its latest release on 2026-10-01. It is not named here; the comparison is about approaches,
not about one project.

This page shows the latest measurement only. Earlier rounds are summarised in one table under
*How Compass got here*; when the benchmark is rerun, update the numbers in place and add a row
there.

## Summary

- **Re-indexing after an edit:** Compass takes **0.17s** on a 613-file repo, the competitor
  **19.6s**. A cold build is 0.58s against 15.1s.
- **Agents:** over 36 tasks in six repos, Sonnet agents with Compass's injected map found the
  right file with **26% fewer tool calls** than plain search and **30% fewer** than the
  competitor, and had the lowest median time on every task set. All three conditions answered
  every task correctly.
- **Tokens:** Compass used the fewest, but only by 1.5 to 1.7%. A fixed ~40k-token session
  overhead (system prompt, tool definitions) dominates every run; the injected map itself is
  350 to 680 tokens.
- **Retrieval:** on the tasks the ranking was tuned on, the correct file is first for 8 of 12.
  On 24 tasks it was never tuned on, it is first for 10, about as often as a `git grep` whose
  keyword was picked by someone who knew the answer.

## How it was measured

Each task is a "where do I change X" prompt in plain language with one known correct file and
symbol, for example "Raise the medium-risk profile's maximum drawdown cap from 15% to 18%".
Agents answered with the file, the symbol and line, and the change, without editing.

There are three task sets of 12:

| Set | Repos | Role |
|---|---|---|
| 1 | This repo (Rust) and a private 613-file Rust + TypeScript trading app | The ranking was tuned on these tasks |
| 2 | A ~200-file TypeScript chat bot and a Go networking project (~40 Go files, plus F# that Compass does not map) | Untuned: written after tuning, by agents never shown the ranking code |
| 3 | A 147-file Java backend and a Python + TypeScript app (51 Python, 36 TypeScript files) | Blind: written after the last ranking change and never used for tuning |

In sets 2 and 3 at least half the tasks describe behaviour instead of naming the file or
symbol. Every answer was checked against the code before the run.

The three conditions:

- **Plain search (base):** Glob, Grep and Read only.
- **Competitor:** built with its code-only extract command, re-indexed with its update command,
  queried with its query command. The agent got the exact instructions the tool's own Claude
  Code installer writes into `CLAUDE.md`, plus its CLI.
- **Compass:** release build. The prompt carried the output of `compass context --query <task>`
  the way the `UserPromptSubmit` hook injects it, plus the CLI for deepening.

Each tool got its own clone so neither indexed the other's cache. Machine: Windows 11, 16
logical cores, NVMe SSD, Defender on.

## Re-index speed

Median wall-clock, process start to exit (what a hook or a person waits for). Compass is the
build with #45 to #52; reading string literals (#52) added about 0.1s to a cold build of the
larger repo and nothing measurable to a re-index.

| | Compass (this repo) | Competitor (this repo) | Compass (613 files) | Competitor (613 files) |
|---|---|---|---|---|
| Cold build | 0.13s | 5.0s | 0.58s | 15.1s |
| Re-index, nothing changed | 0.055s | 5.0s | 0.17s | 20.1s |
| Re-index after 1 edited file | 0.11s | 5.1s | 0.17s | 19.6s |
| Re-index after 10 edited files | 0.13s | 6.0s | 0.18s | 19.5s |
| One query | 0.04s | 0.48s | 0.16s | 1.2s |

The competitor's update re-runs its whole cross-file build and clustering; its cache only skips
re-parsing. Compass's query includes a check that the cached map still matches the working tree
(#45), so it never answers from an outdated map.

## Retrieval

What each tool hands the agent for the task prompt, and where the correct file lands in it:
first / in the top 5 / anywhere, out of 12. Tokens are characters / 4.

| | Set 1 (tuned) | Set 2 (untuned) | Set 3 (blind) | Mean tokens (set 1 / 2 / 3) |
|---|---|---|---|---|
| `git grep -il <keyword>` | 1 / 4 / 10 | 5 / 8 / 12 | 4 / 6 / 12 | 1,162 / 106 / 348 |
| Competitor's query | 2 / 4 / 6 | 3 / 4 / 6 | 2 / 5 / 9 | 1,505 / 1,553 / 1,722 |
| **Compass** | **8 / 11 / 11** | **6 / 8 / 8** | **4 / 6 / 7** | 642 / 358 / 679 |

The grep row is generous: its keyword was chosen by the task writer while looking at the
answer. It finds almost everything because it returns every file containing the word, which is
also why an agent still has to read through its list.

The gap between set 1 and sets 2 and 3 is how much the tuned figure was fitted to its tasks.
What Compass still misses on the blind set is mostly vocabulary: the prompt says "throws away
anything older than half a year" and the code says `cleanup`, or "keep people signed in" for a
refresh-token lifetime. Word matching cannot bridge that; it would take synonyms or embeddings.

## Agents

Sonnet, one run per cell. Tool calls exclude the one call every agent made to read its brief.
Sets 1 and 2 were run with the build with #45 to #49, set 3 with #45 to #52.

| Set | | Total tokens | Tool calls | Median time per task | Total time | Correct |
|---|---|---|---|---|---|---|
| 1 | Plain search | 550,537 | 42 | 12.5s | 155.2s | 12/12 |
| 1 | Competitor | 556,636 | 48 | 14.5s | 223.1s | 12/12 |
| 1 | **Compass** | **543,188** | **28** | **9.8s** | 172.5s | 12/12 |
| 2 | Plain search | 545,966 | 36 | 11.9s | 149.9s | 12/12 |
| 2 | Competitor | 541,896 | 40 | 13.4s | 159.4s | 12/12 |
| 2 | **Compass** | **536,775** | **30** | **10.2s** | **125.4s** | 12/12 |
| 3 | Plain search | 569,901 | 46 | 14.7s | 191.4s | 12/12 |
| 3 | Competitor | 564,563 | 43 | 15.3s | 204.0s | 12/12 |
| 3 | **Compass** | **557,788** | **34** | **13.0s** | **174.1s** | 12/12 |
| All | Plain search | 1,666,404 | 124 | | 496.5s | 36/36 |
| All | Competitor | 1,663,095 | 131 | | 586.5s | 36/36 |
| All | **Compass** | **1,637,751** | **92** | | **472.0s** | 36/36 |

API latency moves single runs a lot. In set 1 one Compass run took 68s for 3 tool calls while
its siblings took 8 to 11s, which is why its total time there is above plain search even though
its median is the lowest. Tool calls are the steadiest signal, and that is where the gap is
largest.

## How Compass got here

Each row is one round of changes, measured the same way. Agent tool calls are on set 1, Compass
against plain search in the same run.

| Build | Set 1 retrieval | Set 2 | Set 3 | Slice tokens (set 1) | Agent tool calls (set 1) |
|---|---|---|---|---|---|
| Before #44: substring matching | 1 / 5 / 8 | not run | not run | 1,712 | 37 vs 41 |
| #44: word and rarity ranking, symbol lines | 6 / 10 / 10 | not run | not run | 553 | 31 vs 45 |
| #45 to #49: fresh map, fields and variants, doc comments, map search | 8 / 10 / 11 | 5 / 7 / 8 | 3 / 6 / 6 | 626 | 28 vs 42 |
| #51, #52: short query words, string literals | 8 / 11 / 11 | 6 / 8 / 8 | 4 / 6 / 7 | 642 | not rerun |

The first build was already the fastest of the three conditions but spent the most tokens: its
~1,700-token slice cost more than it saved. From #44 on it spends the fewest.

What each change does:

- **#44, ranking:** identifiers and paths are split into words, lightly stemmed and matched by
  word or prefix instead of raw substring; filler is dropped; each term is weighted by how rare
  it is. Each listed file shows its matching symbols with their line (`medium_risk:120`).
- **#45, freshness:** the hook checks the cached map against the working tree on every prompt
  and reindexes only after an edit.
- **#46, fields and enum variants** are symbols in 11 languages, and big files no longer win just
  by having many symbols.
- **#47, doc comments:** the comment above each symbol and each file's leading doc become one-line
  summaries used for ranking and shown in the pack.
- **#48, map search:** `compass map` searches by task with the same ranking.
- **#49:** `compass context "some task"` treats text that is not a path as the task.
- **#51, short words:** query words of three letters or fewer count half, so a word like `bot`
  that names one file no longer outranks the answer.
- **#52, string literals and config keys:** short strings and object keys are weak evidence, so
  a task that quotes a message or a setting finds the file that contains it.

## Limitations

- One run per cell. Token and time differences of a few percent are within run-to-run noise;
  the tool-call gap is not.
- 36 tasks, all "find where to change X". Nothing here measures editing or multi-file changes.
- The competitor ran code-only. Its LLM-backed extraction for docs and papers was not tested.
- The re-index rows for the competitor are from the earlier round; its build did not change.
- The latest round was measured on local builds with the open PRs applied, before they were
  merged.
