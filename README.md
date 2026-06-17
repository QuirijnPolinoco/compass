# MapAI

**A local-first tool that maps any codebase into a queryable graph — and serves it to AI
coding assistants over [MCP](https://modelcontextprotocol.io).**

AI assistants waste tokens grepping a whole repo to find the right files, and sometimes
edit the wrong ones or invent paths that don't exist. MapAI gives the human *and* the AI a
shared, accurate map — what files exist, how they connect (imports), and where the
important logic lives — so the assistant goes straight to the right files. The map is
useful on its own to a human, too.

- **Local-first & private** — parsing runs entirely on your machine with [tree-sitter];
  no network calls, no API keys, your code never leaves the box.
- **One binary, zero config** — run it in a repo and you get a map in seconds.
- **Model-agnostic** — it speaks MCP, so it works with any assistant (Claude, Gemini,
  ChatGPT, …) without integrating with any of them.

> **Status:** early. The core engine, the MCP server, and all Tier 1 languages work and
> are tested. Prebuilt release binaries and live (watch-based) freshness are on the
> roadmap — for now, build from source (below).

## Supported languages

Go · Python · Java · C# · TypeScript/JavaScript

Adding a language is a self-contained unit of work behind a stable interface — see
[CONTRIBUTING.md](CONTRIBUTING.md). More languages are on the roadmap in
[`ProjectInfo.md`](ProjectInfo.md).

## Install

**Recommended — download a prebuilt binary.** MapAI is a single self-contained executable:
no runtime, no dependencies. Grab the one for your platform from the
[latest release](https://github.com/QuirijnPolinoco/MapAI/releases/latest), unpack it, and
put `mapai` on your `PATH`:

| Platform | Asset |
|----------|-------|
| Linux (x86-64) | `mapai-x86_64-unknown-linux-gnu.tar.gz` (or `-musl` for a fully static build) |
| macOS (Apple Silicon) | `mapai-aarch64-apple-darwin.tar.gz` |
| macOS (Intel) | `mapai-x86_64-apple-darwin.tar.gz` |
| Windows (x86-64) | `mapai-x86_64-pc-windows-msvc.zip` |

Then run `mapai overview .` in any repository. (A `brew` / `scoop` / `cargo-binstall`
one-liner is on the roadmap.)

## Build from source

Alternatively, build it yourself. Requires a [Rust toolchain](https://rustup.rs) and a C
compiler (for the tree-sitter grammars: MSVC Build Tools on Windows, `cc`/Xcode CLT on
Linux/macOS).

```sh
cargo build --release
# binary at target/release/mapai
```

## Usage

```sh
# A human-readable summary of the repo map
mapai overview path/to/repo

# What a file imports, and what imports it
mapai deps path/to/repo src/main.go

# Imports that point at files that don't exist
mapai broken path/to/repo

# Which languages this build supports
mapai languages

# Run the MCP server over stdio (for an AI host to connect to)
mapai serve path/to/repo
```

### Using it from an AI assistant (MCP)

MapAI is an MCP server over stdio. Point your MCP-capable host at the `mapai serve` command;
for a host that uses a JSON config, the entry looks like:

```json
{
  "mcpServers": {
    "mapai": {
      "command": "mapai",
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

## How it works

1. Walk the repo, respecting `.gitignore`.
2. Detect each file's language (by extension, with a shebang fallback).
3. Parse each file locally with tree-sitter and extract symbols + imports.
4. Resolve imports to real files and build a language-agnostic graph.
5. Serve the graph to humans (CLI) and AI assistants (MCP).

The architecture — a Cargo workspace with a language-agnostic core and one crate per
language behind a stable `Extractor` trait — is documented in
[`docs/architecture/`](docs/architecture/) (requirements, ADRs, and diagrams).

## Contributing

Contributions — especially new languages — are very welcome. Start with
[CONTRIBUTING.md](CONTRIBUTING.md): it covers the repo layout, the living-docs rule, and a
step-by-step checklist for adding a language (copy `crates/mapai-lang-template/` and fill
in the TODOs).

## License

Dual-licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.

[tree-sitter]: https://tree-sitter.github.io/tree-sitter/
