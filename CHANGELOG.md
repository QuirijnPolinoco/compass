# Changelog

All notable changes to Compass are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) (pre-1.0: minor versions may
contain breaking changes).

## [Unreleased]

### Added

- TypeScript/JS: imports of **in-repo packages by name** (`@acme/shared` in an npm, pnpm, Yarn
  or Lerna workspace) now resolve to the package's source, so dependencies and blast radius
  cross package boundaries. `package.json` is read as resolver input; build-output entries
  (`dist/…`) are followed to their `src/` counterpart.
- **HTML** language extractor: `href`/`src` references to mapped stylesheets, scripts and pages
  become edges; element ids are symbols. A miss is external, never a broken import.
- **CSS** language extractor: `@import` edges between stylesheets; `.class`, `#id`,
  `--custom-property` and `@keyframes` names as symbols.
- Call edges from the Kotlin, Ruby, PHP, C and C++ extractors — every supported language now
  contributes to the call graph.

### Fixed

- The visual map froze for tens of seconds when **Symbols** was switched on. Only files go
  through the force layout now; each file's symbols are placed in a cloud around it, instantly.
- After upgrading Compass, files that had not changed kept the *previous* release's extraction
  (for example no call edges for an untouched TypeScript file after 0.7 → 0.8). Caches are now
  stamped with the release that wrote them and rebuilt on a mismatch.
- A build compiled without a language no longer maps that language's files from a cache written
  by a fuller build.

### Changed

- `compass context` and `compass guard` now report a file's **transitive** blast radius (every
  file a change can reach), not just its direct importers.
- Internal: `compass-cli` is split into per-command modules.

## [0.8.0] - 2026-09-17

### Added

- **R** language extractor: functions, R6/S4/Reference classes and methods, S4 generics, and
  `source()` / `sys.source()` / `here::here()` resolution.
- **C++** language extractor: classes, structs, unions, enums, namespaces, functions, and
  quoted-`#include` resolution.
- Symbol→symbol **call edges** from the TypeScript/JS, Python, Go, Java, C# and R extractors
  (previously Rust only). Only calls that can be named without type information are emitted.
- MCP tools: `impact` ("what breaks if I change this file?"), `find_symbol`, `symbol_calls`
  (callers and callees) and `supported_languages`.
- `compass --version` (`-V`, `version`).
- TypeScript/JS: `const f = () => {}` and `const f = function () {}` are now function symbols.

## [0.7.0] - 2026-06-26

### Added

- One-line cross-platform installer (`curl | sh` / `irm | iex`).
- `compass install` — wire Compass into AI hosts (Claude Code, Cursor, Codex).
- `compass audit` — graph-driven health checks.
- `compass guard` — opt-in PreToolUse hook that asks before edits to high-impact files.
- Local token-savings dashboard in the visual map.
- MCP tools: `graph_stats`, `hubs`, `get_community`.
- Import and call edges are tagged with a confidence: **Resolved** or **Heuristic**.

## [0.6.0] - 2026-06-23

### Added

- Symbol→symbol call graph (Rust extractor first).
- Broader import resolution across Go, Python, Java and C#.

### Changed

- Incremental indexing: unchanged files are no longer re-read.

## [0.5.0] - 2026-06-23

### Added

- `compass map` — a live, interactive visual map served on localhost (`compass-viz`).
- `compass context` — prompt pre-injection (ADR-0006), with a zero-script Claude Code hook and
  a session graph that avoids re-injecting files already shown.
- `/compass` slash command for Claude Code.
- MCP tools: `subgraph`, `shortest_path`; community clustering in the core graph.
- Rust: cross-crate `use` and fully-qualified path resolution via the Cargo workspace.
- TypeScript: tsconfig path aliases, `.js`→`.ts` specifiers, `require()` and dynamic `import()`.
- Release checksums, `cargo-binstall` metadata, Homebrew and Scoop packaging.

### Fixed

- Visual map: instant-fit layout on first load and faster scroll-zoom.

## [0.4.0] - 2026-06-17

### Added

- `compass init` — one-command project setup.

## [0.3.0] - 2026-06-17

### Added

- Tier 2 language extractors: Rust, Kotlin, Ruby, PHP and C.
- `compass watch` — live re-mapping as files change.

## [0.1.0] - 2026-06-17

### Added

- Rust workspace with a language-agnostic core graph and the stable `Extractor` interface.
- Tier 1 language extractors: Go, Python, Java, C# and TypeScript/JavaScript.
- MCP server over stdio with `overview`, `file_dependencies` and `broken_imports` tools.
- Most-connected files in the overview.

[Unreleased]: https://github.com/QuirijnPolinoco/compass/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/QuirijnPolinoco/compass/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/QuirijnPolinoco/compass/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/QuirijnPolinoco/compass/compare/v0.4.0...v0.6.0
[0.5.0]: https://github.com/QuirijnPolinoco/compass/compare/v0.4.0...v0.6.0
[0.4.0]: https://github.com/QuirijnPolinoco/compass/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/QuirijnPolinoco/compass/compare/v0.1.0...v0.3.0
[0.1.0]: https://github.com/QuirijnPolinoco/compass/releases/tag/v0.1.0
