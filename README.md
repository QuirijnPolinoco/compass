# Compass

**A local-first tool that maps any codebase into a queryable graph — and serves it to AI
coding assistants over [MCP](https://modelcontextprotocol.io).**

AI assistants waste tokens grepping a whole repo to find the right files, and sometimes
edit the wrong ones or invent paths that don't exist. Compass gives the human *and* the AI a
shared, accurate map — what files exist, how they connect (imports), and where the
important logic lives — so the assistant goes straight to the right files. The map is
useful on its own to a human, too.

- **Local-first & private** — parsing runs entirely on your machine with [tree-sitter];
  no network calls, no API keys, your code never leaves the box.
- **One binary, zero config** — run it in a repo and you get a map in seconds.
- **Model-agnostic** — it speaks MCP, so it works with any assistant (Claude, Gemini,
  ChatGPT, …) without integrating with any of them.

> **Status:** early but functional. The core engine, the MCP server, 10 languages, live
> re-mapping (`compass watch`), and an interactive **visual map** (`compass map`) work and are
> tested. Prebuilt binaries are on the
> [releases page](https://github.com/QuirijnPolinoco/Compass/releases).

## Quick start

```sh
# 1. Install (one time)
cargo install --path crates/compass-cli      # or grab a binary from Releases

# 2. In your project — build the map AND enable your AI assistant, in one step:
compass init
```

`compass init` indexes the repo (creating `.compass/`, or refreshing it if it already
exists) and writes a `.mcp.json` so any MCP-capable assistant (Claude Code, …) uses Compass
automatically. It's idempotent — re-run it anytime.

## Supported languages

Go · Python · Java · C# · TypeScript/JavaScript · Rust · Kotlin · Ruby · PHP · C

Adding a language is a self-contained unit of work behind a stable interface — see
[CONTRIBUTING.md](CONTRIBUTING.md). More languages are on the roadmap in
[`ProjectInfo.md`](ProjectInfo.md).

## Install

Compass is one self-contained binary. Building from source with Rust works on every OS today;
a prebuilt download is offered when a release is available.

### 1. Prerequisites

A **Rust toolchain** and a **C compiler** (the tree-sitter grammars are C). Per OS:

| OS | Rust | C compiler |
|----|------|-----------|
| **Windows** | [rustup](https://rustup.rs) (the `x86_64-pc-windows-msvc` default) | **Microsoft C++ Build Tools** — installer → "Desktop development with C++" |
| **macOS** | [rustup](https://rustup.rs) | Xcode CLT: `xcode-select --install` |
| **Linux** | [rustup](https://rustup.rs) | `sudo apt install build-essential` (Debian/Ubuntu) · `sudo dnf groupinstall "Development Tools"` (Fedora) |

Verify: `cargo --version` should print **1.85 or newer**.

### 2. Build & install from source (works today)

From the repo root — same command on every OS:

```sh
cargo install --path crates/compass-cli
```

This compiles the release binary (~1–2 min the first time; it builds all 10 grammars) and
installs `compass` into Cargo's bin dir, which rustup already adds to your `PATH`:

- **Windows:** `%USERPROFILE%\.cargo\bin\compass.exe`
- **macOS / Linux:** `~/.cargo/bin/compass`

Open a **new** terminal and verify:

```sh
compass --help
compass languages      # should list all 10 languages
```

If `compass` isn't found, add that Cargo bin dir to your `PATH` and reopen the terminal.

> **Just trying it out?** Skip the install and run any command from the repo root with
> `cargo run -p compass-cli -- <args>` (e.g. `cargo run -p compass-cli -- map .`).

### 3. Prebuilt binary (if a release is available)

No runtime, no dependencies — grab your platform's asset from the
[latest release](https://github.com/QuirijnPolinoco/Compass/releases/latest), unpack it, and
put `compass` on your `PATH`:

| Platform | Asset |
|----------|-------|
| Linux (x86-64) | `compass-x86_64-unknown-linux-gnu.tar.gz` (or `-musl` for a fully static build) |
| macOS (Apple Silicon) | `compass-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `compass-x86_64-apple-darwin.tar.gz` |
| Windows (x86-64) | `compass-x86_64-pc-windows-msvc.zip` |

(A `brew` / `scoop` / `cargo-binstall` one-liner is on the roadmap.)

## Usage

```sh
# Set up a repo in one step: build the map + enable MCP (start here)
compass init path/to/repo

# A human-readable summary of the repo map
compass overview path/to/repo

# Open an interactive, live-updating visual map in your browser
compass map path/to/repo

# What a file imports, and what imports it
compass deps path/to/repo src/main.go

# Imports that point at files that don't exist
compass broken path/to/repo

# Keep the map fresh automatically as you edit (Ctrl+C to stop)
compass watch path/to/repo

# Print the most relevant slice of the map for a task — to pre-inject into an AI prompt
compass context path/to/repo --query "add a retry to the HTTP client"

# Which languages this build supports
compass languages

# Run the MCP server over stdio (for an AI host to connect to)
compass serve path/to/repo
```

### The visual map

`compass map` opens an interactive, force-directed picture of your repo in the browser —
files are nodes, imports are edges — and **keeps it live as you edit** (it re-lays-out in
place, no refresh). It's the human-readable counterpart to what the AI sees over MCP.

- **Grouped by sub-part, not by folder.** Nodes are colored by *detected community* — files
  that depend on each other cluster together — so the map shows cohesive "parts of the
  project" whether your repo is organized by feature or by type. Shared utility files (hubs)
  render neutral. You can also color by folder or by language with one click.
- **Files or symbols.** Defaults to a file-level graph; toggle **Symbols** to expand into
  functions/classes. Search, zoom-to-reveal labels, and node sizes scaled by connectivity.
- **Local-first.** The server binds **`127.0.0.1` only**, is read-only, and lives only while
  the command runs. The renderer is embedded, so the map works with no internet.

```sh
compass map                 # serve the live map + open the browser
compass map --port 8123     # pick a port (default is an uncommon high one, 62049)
compass map --no-open       # just print the URL
compass map --snapshot      # write a self-contained .compass/map.html and exit
```

> It defaults to an uncommon high port (`62049`) and automatically falls back to a free one
> if that's busy, so it won't collide with your other servers or containers.

### Using it from an AI assistant

There are two complementary ways to give an AI the map ([ADR-0006](docs/architecture/decisions/0006-context-pre-injection.md)):

**1. Pre-injection (the efficient default).** Inject the relevant slice of the map into the
prompt *before* the AI reasons, so it goes straight to the right files instead of exploring.
For **Claude Code**, add this `UserPromptSubmit` hook to your project's `.claude/settings.json`:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      { "hooks": [ { "type": "command", "command": "compass context --hook" } ] }
    ]
  }
}
```

`compass context --hook` reads the prompt from stdin, ranks the most relevant files, and prints
a compact map slice Claude Code adds to the context — no scripting, cross-platform, and
failure-safe. See [`integrations/claude-code/`](integrations/claude-code/). (A benchmark found
this is where the token savings actually are — [`docs/benchmarks/`](docs/benchmarks/README.md).)

**2. MCP tools (deepening + any other host).** `compass init` writes the `.mcp.json` below;
keep it for on-demand deepening (`subgraph`, `shortest_path`, …) when the pre-injected slice
misses, and as the universal path for non-Claude hosts. To wire it up by hand — Compass is an
MCP server over stdio:

```json
{
  "mcpServers": {
    "compass": {
      "command": "compass",
      "args": ["serve", "path/to/repo"]
    }
  }
}
```

The server exposes these tools:

| Tool | What it returns |
|------|-----------------|
| `overview` | File/symbol/import counts, per-language breakdown, most-connected files |
| `file_dependencies` | What a given file imports and what imports it |
| `broken_imports` | Imports that resolve to no real file |
| `subgraph` | The neighborhood around a file (deps + dependents within N hops) — a small, cheap slice instead of grepping the repo |
| `shortest_path` | The import chain connecting two files ("what connects X to Y") |

## How it works

1. Walk the repo, respecting `.gitignore`.
2. Detect each file's language (by extension, with a shebang fallback).
3. Parse each file locally with tree-sitter and extract symbols + imports.
4. Resolve imports to real files and build a language-agnostic graph.
5. Serve that one graph three ways: a human **visual map** (`compass map`), AI **pre-injection**
   (`compass context`), and **MCP** tools (`compass serve`).

The architecture — a Cargo workspace with a language-agnostic core and one crate per
language behind a stable `Extractor` trait — is documented in
[`docs/architecture/`](docs/architecture/) (requirements, ADRs, and diagrams).

## Contributing

Contributions — especially new languages — are very welcome. Start with
[CONTRIBUTING.md](CONTRIBUTING.md): it covers the repo layout, the living-docs rule, and a
step-by-step checklist for adding a language (copy `crates/compass-lang-template/` and fill
in the TODOs).

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.

[tree-sitter]: https://tree-sitter.github.io/tree-sitter/
