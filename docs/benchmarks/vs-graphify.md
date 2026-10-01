# Compass vs graphify vs plain grep

> Question: which gets an AI agent to the right file fastest and cheapest, and which keeps its
> map fresh fastest after an edit? Every number below was measured on the machine noted. Where a
> result does not favour Compass, it is reported anyway.

## TL;DR

- **Re-indexing after an edit:** Compass takes **0.15s** on a 613-file repo, graphify takes
  **33 to 39s**. That is roughly 250x. A cold build is 0.45s against 26s.
- **Agents:** with the improved `compass context` injection, Sonnet agents reached the right
  file and line with **31% fewer tool calls** and in **30% less wall time** than plain grep, and
  34% / 35% fewer than graphify. All three conditions answered 12/12 correctly.
- **Tokens:** the agent-level token saving is small (**1.6%** vs grep, 0.5% vs graphify),
  because a fixed ~40k-token session overhead (system prompt, tool definitions) dominates every
  run. The injected map itself shrank from ~1,700 to ~550 tokens.
- **Retrieval:** the correct file is in Compass's top 5 for **10 of 12** tasks (top 1 for 6),
  against 4 of 12 for graphify's `query` and 4 of 12 for a first `git grep`.

## Setup

- **Subjects:** this repo (199 mapped files, Rust) and a private 613-file Rust + TypeScript
  trading app. Each tool got its own clone so neither indexed the other's cache.
- **graphify:** `graphifyy` 0.9.73, code-only (`graphify extract --code-only`, no LLM),
  `graphify update .` for re-indexing, `graphify query` for retrieval. The graphify condition got
  the exact CLAUDE.md section `graphify claude install` writes.
- **Compass:** release build. The prompt carried the output of `compass context --query <task>`
  the way the `UserPromptSubmit` hook injects it, plus the CLI for deepening.
- **Base:** Glob, Grep and Read only.
- **Tasks (12):** "where do I change X" prompts in plain language, each with one known correct
  file and symbol, for example "Raise the medium-risk profile's maximum drawdown cap from 15% to
  18%" or "Add bun.lockb to the files that are mapped but never analysed". Agents had to answer
  with the file, the symbol and line, and the change, without editing.
- **Machine:** Windows 11, 16 logical cores, NVMe SSD, Defender on.

## Re-index speed

Median wall-clock, process start to exit (what a hook or a person waits for).

| | Compass (this repo) | graphify (this repo) | Compass (613 files) | graphify (613 files) |
|---|---|---|---|---|
| Cold build | 0.085s | 7.0s | 0.45s | 25.8s |
| Re-index, nothing changed | 0.044s | 6.7s | 0.13s | 34.1s |
| Re-index after 1 edited file | 0.084s | 6.7s | 0.15s | 39.2s |
| Re-index after 10 edited files | 0.091s | 7.9s | 0.15s | 33.0s |
| One query | 0.012s | 0.67s | 0.034s | 1.76s |

graphify's `update` re-runs its whole cross-file build and clustering in Python; its cache only
skips re-parsing. `--no-cluster` still took 30s on the larger repo.

## Retrieval (deterministic)

What each tool hands the agent for the task prompt, and where the correct file lands in it.
Tokens are characters / 4.

| | Top 1 | Top 5 | Found at all | Mean tokens |
|---|---|---|---|---|
| `git grep -il <keyword>` | 1/12 | 4/12 | 10/12 | 1,162 |
| `graphify query` | 2/12 | 4/12 | 6/12 | 1,504 |
| `compass context` before | 1/12 | 5/12 | 8/12 | 1,712 |
| **`compass context` now** | **6/12** | **10/12** | **10/12** | **553** |

The two tasks Compass still misses share no words with their target ("the panel where a user
switches on real-money trading" for `LiveEnablePanel.tsx`). No lexical ranker bridges that, and
graphify misses them too.

## Agents (live A/B)

Twelve tasks, three conditions, Sonnet, one run per cell per round. Round 2 ran all three
conditions together after the ranking change, so it is the like-for-like comparison. Tool calls
exclude the one call every agent made to read its brief.

| Round 2 | Total tokens | Tool calls | Wall time | Correct |
|---|---|---|---|---|
| Base | 558,537 | 45 | 246.8s | 12/12 |
| graphify | 552,075 | 47 | 265.1s | 12/12 |
| **Compass** | **549,396** | **31** | **172.7s** | 12/12 |

Round 1, with the old injection: base 566,672 tokens / 41 calls / 163.9s, graphify 551,724 / 46 /
201.5s, Compass 581,597 / 37 / 139.8s. The old Compass was already the fastest, but it spent
the most tokens, because its ~1,700-token slice cost more than it saved. Shrinking and
sharpening the slice is what turned that around.

## What changed in Compass

- **Ranking** (`compass-core/src/rank.rs`): identifiers and paths are split into words
  (snake, kebab and camel case), lightly stemmed, and matched by word or prefix instead of raw
  substring, so `cap` no longer matches `escape`. English filler is dropped. Each term is weighted
  by inverse document frequency, so `drawdown` outweighs `risk` in a repo that is all about risk.
  Files that cover more of the distinct terms rank higher. Tests, sentence-like test-case names,
  and CSS/HTML are demoted unless the prompt is about them. Files scoring below 30% of the best
  are dropped.
- **Output:** each file lists the symbols that matched, with their line (`medium_risk:120`), so
  the agent can open the file at the right place. Import and importer lists show three paths and
  a count. The default cap is 8 files instead of 12.

## Limitations

- n = 1 per cell per round; API latency drifted between rounds (base was ~50% slower in round
  2), so compare within a round.
- 12 tasks, all "find where to change X". Structural questions (blast radius, centrality) are in
  [README.md](README.md).
- graphify ran code-only. Its LLM-backed semantic extraction for docs and papers was not tested.
- Struct fields and enum variants are not symbols yet, so a value that lives in a field (the
  drawdown cap) ranks its file lower than it should.
