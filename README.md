# 🧭 Compass

**A local-first map of your codebase — for your AI _and_ for you.**

Compass parses your repo into a queryable graph and feeds the *relevant slice* straight into
your AI assistant's prompt — so it stops grepping the whole tree and starts reasoning. The same
graph powers a **live, interactive map** you can open in the browser.

![license](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)
![platforms](https://img.shields.io/badge/platforms-Windows%20%C2%B7%20macOS%20%C2%B7%20Linux-informational)
![built with Rust](https://img.shields.io/badge/built%20with-Rust-orange)
![languages mapped](https://img.shields.io/badge/languages%20mapped-10-success)
![local-first](https://img.shields.io/badge/local--first-no%20network-success)

```sh
compass init     # map the repo + wire up your AI assistant
compass map      # open the live visual map in your browser
```

* * *

## What is Compass?

AI coding assistants waste tokens grepping a whole repo to find the right files — and sometimes
edit the wrong ones or invent paths that don't exist. Compass gives the human **and** the AI a
shared, accurate map: what files exist, how they connect (imports), and where the important logic
lives. Then it delivers that map two ways:

- **To your AI** — by *pre-injecting* the relevant slice into each prompt (and an MCP server for
  deeper, on-demand queries). Your AI goes straight to the right files.
- **To you** — as an interactive, force-directed graph in the browser that updates live as you edit.

It's **local-first** (parsing runs on your machine with [tree-sitter] — no network, no API keys,
your code never leaves the box), **one binary, zero config**, and **model-agnostic**.

> **Status:** early but functional. The engine, MCP server, 10 languages, live re-mapping, the
> visual map, and prompt pre-injection all work and are tested.

* * *

## Results

In a **controlled A/B** on a real ~280-file Rust + TypeScript workspace (one strong agent, 10
tasks, identical prompts/tools — only the delivery differed), pre-injecting Compass's map slice
**cut the agent's tool calls ~31%** vs. both plain grep and calling Compass as an MCP tool — while
keeping accuracy.

| Delivery | Tool calls to locate (10 tasks) |
|----------|---------------------------------|
| grep only | 66 |
| Compass as an MCP tool the agent calls | 65 |
| **Compass pre-injection** | **45 (≈31% fewer)** |

Honest by design: the MCP tool-loop alone was a wash (calling a tool adds turns), and on simple
symbol lookups a capable agent + grep is already fine. Full methodology, per-task numbers, and
caveats (n=1, self-reported, etc.) — including where Compass *doesn't* win — are in
**[docs/benchmarks](docs/benchmarks/README.md)**.

* * *

## Supported AI tools

| Assistant | How Compass plugs in |
|-----------|----------------------|
| **Claude Code** | `UserPromptSubmit` hook for **pre-injection** (the efficient default) + MCP for deepening — see [integrations/claude-code](integrations/claude-code/) |
| **Cursor · Windsurf · any MCP host** | MCP server over stdio (`compass serve`) |
| **Anything else** | pipe `compass context` output into the prompt yourself |

* * *

## Supported languages

| | | | | |
|---|---|---|---|---|
| Go | Python | Java | C# | TypeScript/JS |
| Rust | Kotlin | Ruby | PHP | C |

Adding a language is a self-contained unit of work behind a stable interface — see
[CONTRIBUTING.md](CONTRIBUTING.md). More are on the roadmap in [`ProjectInfo.md`](ProjectInfo.md).

* * *

## Install

Compass is one self-contained binary. Building from source works on every OS today; a prebuilt
download is offered when a release is available.

### 1. Prerequisites — Rust + a C compiler (for the tree-sitter grammars)

| OS | Rust | C compiler |
|----|------|-----------|
| **Windows** | [rustup](https://rustup.rs) | **Microsoft C++ Build Tools** → "Desktop development with C++" |
| **macOS** | [rustup](https://rustup.rs) | `xcode-select --install` |
| **Linux** | [rustup](https://rustup.rs) | `sudo apt install build-essential` · `sudo dnf groupinstall "Development Tools"` |

`cargo --version` should print **1.85+**.

### 2. Build & install (same on every OS)

```sh
cargo install --path crates/compass-cli
```

Installs `compass` into Cargo's bin dir — `%USERPROFILE%\.cargo\bin` (Windows) or `~/.cargo/bin`
(macOS/Linux), already on your `PATH` via rustup. Open a new terminal and check:

```sh
compass --help
compass languages      # lists all 10 languages
```

> **Just trying it?** From the repo root: `cargo run -p compass-cli -- map .` (no install).

### 3. Prebuilt binary (if a release exists)

Grab your platform's asset from the [latest release](https://github.com/QuirijnPolinoco/compass/releases/latest),
unpack, and put `compass` on your `PATH`. (`brew`/`scoop`/`cargo-binstall` are on the roadmap.)

* * *

## Quickstart

```sh
cd your/project
compass init     # build the map (.compass/) + write .mcp.json for MCP hosts
compass map      # open the live visual map (auto-picks a free localhost port)
```

`compass init` is idempotent — re-run it anytime. Common commands:

| Command | What it does |
|---------|--------------|
| `compass init` | Map the repo + wire up MCP |
| `compass map` | Live, interactive visual map in the browser (`--snapshot` for a static `.html`) |
| `compass context --query "…"` | The relevant map slice to pre-inject into a prompt |
| `compass overview` | Human-readable summary (files, languages, most-connected) |
| `compass deps <file>` | What a file imports and what imports it |
| `compass broken` | Imports that resolve to no real file |
| `compass watch` | Re-map automatically as you edit |
| `compass serve` | Run the MCP server over stdio |

* * *

## Make your assistant always use the map

The biggest win is **pre-injection** — give the AI the right context *before* it starts, so it
never wastes turns exploring. For **Claude Code**, drop this into your project's
`.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      { "hooks": [ { "type": "command", "command": "compass context --hook" } ] }
    ]
  }
}
```

`compass context --hook` reads the prompt from stdin, ranks the most relevant files, and prints a
compact map slice Claude Code adds to the context — no scripting, cross-platform, failure-safe,
and fast (it uses the `.compass/` cache). Details + the `/compass` slash command:
[integrations/claude-code](integrations/claude-code/).

For other hosts, `compass init` registers the MCP server, exposing these tools:

| Tool | Returns |
|------|---------|
| `overview` | File/symbol/import counts, per-language breakdown, most-connected files |
| `file_dependencies` | What a file imports and what imports it |
| `subgraph` | The neighborhood around a file — a small, cheap slice instead of grepping |
| `shortest_path` | The import chain connecting two files |
| `broken_imports` | Imports that resolve to no real file |

* * *

## The visual map

`compass map` opens a force-directed picture of your repo — files are nodes, imports are edges —
that **updates live as you edit** (it re-lays-out in place, no refresh).

- **Grouped by sub-part, not folder.** Nodes are colored by *detected community* (files that
  depend on each other), so cohesive parts of the project pop — whether you organize by feature
  or by type. Switch to color-by-folder or by-language with one click; shared hubs render neutral.
- **Files or symbols.** A file-level graph by default; toggle **Symbols** to expand into
  functions/classes. Plus search, zoom-to-reveal labels, and node sizes scaled by connectivity.
- **Local-first.** The server binds **`127.0.0.1` only**, is read-only, lives only while the
  command runs, and the renderer is embedded — so it works with no internet.

* * *

## How it works

1. Walk the repo, respecting `.gitignore`.
2. Detect each file's language (by extension, with a shebang fallback).
3. Parse each file locally with [tree-sitter] and extract symbols + imports.
4. Resolve imports to real files and build one language-agnostic graph.
5. Serve that graph three ways: the **visual map** (`compass map`), **pre-injection**
   (`compass context`), and **MCP** tools (`compass serve`).

The architecture — a Cargo workspace with a language-agnostic core and one crate per language
behind a stable `Extractor` trait — is documented in [`docs/architecture/`](docs/architecture/)
(requirements, ADRs, and diagrams).

* * *

## Privacy

100% local. Parsing runs on your machine; **no network calls, no telemetry, no API keys**; your
code is only ever *read*, never executed or uploaded. The map lives on disk under `.compass/`.
The visual-map server is loopback-only and read-only.

* * *

## Contributing

Contributions — especially new languages — are very welcome. Start with
[CONTRIBUTING.md](CONTRIBUTING.md): repo layout, the living-docs rule, and a step-by-step
checklist for adding a language (copy `crates/compass-lang-template/` and fill in the TODOs).

## License

Dual-licensed under either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.

[tree-sitter]: https://tree-sitter.github.io/tree-sitter/
