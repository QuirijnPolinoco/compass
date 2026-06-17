MapAI — Project Spec

A local-first repo-mapping MCP server.

1. Project Description

MapAI is a local-first tool that maps any codebase into a queryable graph and serves it to AI coding assistants over MCP (Model Context Protocol).

Today, AI assistants waste tokens grepping through a whole repo to find the right files, and sometimes edit the wrong ones or invent paths that don't exist. MapAI gives both the human and the AI a shared, accurate map — what files exist, how they connect (imports, calls), and where the important logic lives — so the AI goes straight to the correct files.

What makes it different

Tools like this already exist (e.g. graphify), but they are heavy to set up — multiple installers, config flags, optional extras, per-platform steps. MapAI's core bet is radical simplicity: one command, zero config, one cross-platform binary, no API keys, code never leaves the machine.

Who it's for

Anyone who works on a codebase and needs to understand it — two overlapping audiences:

AI users who want their assistant to navigate the project accurately, without burning tokens or inventing wrong paths.
Anyone who needs an overview — new joiners, solo devs returning to an old project, or anyone trying to understand how a codebase fits together — whether or not they use AI at all.

The map is useful on its own for a human; feeding it to an AI is an added benefit, not a requirement.

How it works (high level)

Walk the project files (respect .gitignore).
Detect each file's language (extension-based, with shebang fallback).
Parse each file locally with tree-sitter to extract symbols and imports.
Build a graph (file → imports → file; symbol → calls → symbol).
Expose the graph via an MCP server so any AI tool can query it.

Because parsing is local and the AI only ever reads the output, MapAI works with any model — Claude, Gemini, ChatGPT, Grok, DeepSeek, Llama — without integrating with any of them individually.

Out of scope for v1

SQL / database-schema mapping (valuable, but a separate extractor — roadmap, not v1).
Cloud sync, dashboards, telemetry.

2. North Star: language support is the core unit of growth

Every future feature should make adding the next language easier, or add one.

MapAI is architected around a pluggable language-extractor interface. Adding a language must be a self-contained unit of work that does not touch the core graph, the MCP layer, or any other language. Core code stays language-agnostic; languages are plugins behind a stable interface.

This principle drives all architectural decisions. If a design choice makes adding language #16 harder, it's the wrong choice.

Language roadmap (by difficulty / ROI)

Tier 1 (v1): Go, Python, Java, C#, TypeScript/JavaScript
Tier 2: Rust, Kotlin, Ruby, PHP, C
Tier 3 (hard): C++, Swift, F#
Web assets (in scope): HTML/CSS — maps a different kind of relationship (links and references, not function calls): <link>/<script>/<a> references between files, asset links, and CSS class/ID usage across HTML. Built on the same extractor interface, just emitting reference-type edges.
Separate extractor (later): SQL — table/column/foreign-key graph, distinct from code.

3. User Stories

Prioritised with MoSCoW: Must = no release without it · Should = important, next · Could = nice-to-have · Won't = explicitly not this release.
Stories are grouped by epic for development; the tag is the priority.

Epic A — Install & Setup

A1 Must As a user, I want to install and run the tool with one command and zero config, so that I get a map in minutes without reading docs.
A2 Must As a user, I want it to be one binary that runs the same on Windows, Mac, and Linux, so that my team gets identical results with no environment setup.
A3 Should As a user, I want it to auto-respect .gitignore, so that build artifacts and dependencies aren't mapped.

Epic B — The Map (human side)

B1 Must As a user, I want a clear overview of my project's structure and how files connect, so that I can navigate where everything is.
B2 Should As a user, I want to see what a file depends on and what depends on it, so that I know what I'll affect before changing it.
B3 Could As a user, I want to spot the most-connected files, so that I know where the important logic lives.

Epic C — AI / MCP Integration

C1 Must As a user, I want the AI to read the same map over MCP, so that it finds the right files without burning tokens grepping the whole repo.
C2 Must As a user, I want the map to work with any major model (Claude, Gemini, ChatGPT, Grok, DeepSeek, Llama), so that I'm never locked into one assistant.
C3 Should As a user, I want the AI to fetch only the relevant subgraph for a task, so that context stays small and cheap.

Epic D — Correctness & Safety

D1 Must As a user, I want the AI to edit only files that exist in the map, so that it doesn't link to or invent paths that don't exist.
D2 Should As a user, I want the tool to flag broken imports or references to missing files, so that I catch mistakes (mine or the AI's) early.

Epic E — Querying

E1 Could As a user, I want to ask "what connects X to Y" and get the path between them, so that I can trace how parts of the system relate.
E2 Could As a user, I want to ask "what breaks if I change this?", so that I can plan refactors safely.

Epic F — Freshness

F1 Should As a user, I want the map to update live as I edit my code (in real time, as changes happen), so that it's always current and never misleads me or the AI.

Epic G — Privacy / Local-First

G1 Must As a user, I want mapping to run fully locally with no API key, so that my proprietary code never leaves my machine.

Epic H — Language Extensibility (the north star, made concrete)

H1 Must As a developer, I want to add a new language by implementing one extractor against a stable interface — without touching core graph or MCP code — so that growing language support stays low-risk and fast.
H2 Should As a user, I want a single source-of-truth list of supported languages the tool reports, so that I know what coverage I have.
H3 Should As a maintainer, I want per-language test fixtures, so that adding or changing a language can't silently break another.
H4 Could As a user, I want more languages added over successive releases (Tier 2/3, then HTML/CSS), so that coverage keeps growing beyond the Tier 1 set.

Won't have (this release)

Won't SQL / database-schema extractor (separate graph model — later roadmap).
Won't Cloud sync, hosted dashboards, telemetry.

4. Definition of Done — adding a new language

A language is "supported" only when all of these are true:

Detection — its file extensions map to the language (plus shebang fallback where relevant).
Parsing — a tree-sitter grammar is wired in and extracts symbols (functions, classes, etc.).
Import resolution — the extractor resolves that language's import/include mechanism to real files in the repo.
Graph output — produces the same node/edge shape as every other language (no special-casing downstream).
Fixtures + tests — a sample project under tests/fixtures/<lang>/ with tests asserting the expected nodes and edges.
No core changes — implemented entirely behind the extractor interface; core and other languages untouched.
Docs — added to the supported-languages list and the roadmap updated.

If a change can't meet #4 and #6, the extractor interface needs fixing first — that's a higher priority than the language itself.

5. First release = all Must stories

The first release is defined by the Must-haves above:

A1, A2 — one-command, cross-platform install.
B1 — human-readable map.
C1, C2 — MCP server any model can query.
D1 — AI edits only mapped, real files.
G1 — fully local, no API key.
H1 — extractor interface in place, with Tier 1 languages (Go, Python, Java, C#, TS/JS).
