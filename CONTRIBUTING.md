# Contributing to Compass

Thanks for helping map the world's codebases. This document is the **project rulebook** —
how the repo is organized, how docs stay trustworthy, how commits look, and how to add a
language (the most common and most valued contribution).

Read it once; it's short on purpose.

---

## 1. Guiding principles

1. **Radical simplicity.** One command, zero config, one binary, no API keys, code never
   leaves the machine. Every change is weighed against this.
2. **The North Star: language support is the unit of growth.** Adding a language must be a
   self-contained unit of work behind the stable extractor interface — it must **not**
   touch the core graph, the MCP layer, or any other language. If a change makes adding the
   *next* language harder, it's the wrong change. (See
   [`docs/architecture/decisions/0002-…`](docs/architecture/decisions/0002-pluggable-language-extractor-architecture.md).)
3. **The core stays language-agnostic.** `compass-core`, `compass-mcp`, and `compass-viz` never
   learn about a specific language.

---

## 2. Repository structure — one folder, one job

Every folder is dedicated to **one** part of the tool. Put code where it belongs; don't
reach across boundaries.

| Folder / crate | What goes here | What must NOT go here |
|----------------|----------------|------------------------|
| `crates/compass-core` | Graph model, node/edge types, `LanguageId`, `Diagnostic`, query engine + query port | Anything language-specific; anything about MCP, tree-sitter, or walking |
| `crates/compass-extract` | The stable `Extractor` trait (`extract` + `resolve`), tree-sitter harness, `RawImport`, `ResolutionContext`, `Detection`, registry | Logic for a particular language |
| `crates/compass-engine` | Orchestration as modules: `walk` (+ `.gitignore` + detection), `index`, `cache`, `config` (`watch` is post-v1) | Per-language parsing/resolution rules; MCP logic |
| `crates/compass-mcp` | MCP server + tool definitions + `schemars` DTOs | Knowledge of any specific language; the engine (it depends on core's query port only) |
| `crates/compass-viz` | Visual-map server (`tiny_http`, 127.0.0.1) + Cytoscape render DTOs + embedded front-end assets | Knowledge of any specific language; the engine (it depends on core's query port only) |
| `crates/compass-cli` | The `compass` binary + composition root: `register_all()` + which languages compile in | Business logic that belongs in a library crate |
| `crates/compass-lang-<name>` | **Everything** for one language: detection, grammar, extraction, import resolution, **plus its own `tests/fixtures/` + snapshot tests** | Any reference to another `compass-lang-*` crate |
| `crates/compass-lang-template` | Copy-paste skeleton for a new language (excluded from the workspace) | Real language logic |
| `tests/e2e` | The single cross-crate smoke test (walk → graph → MCP overview) | Per-language fixtures (those live in each lang crate) |
| `docs/architecture` | Requirements, architecture, ADRs, diagrams | — |

If you're unsure where something goes, it usually means a boundary needs clarifying — open
an issue and ask. Asking is always welcome.

---

## 3. Documentation rules — docs are living

- **Docs change with the code, in the same PR.** A PR that changes behavior and not the
  docs is incomplete.
- **Remove what's no longer true.** Stale docs are worse than none. If a section stops
  being relevant, delete it — don't leave it "just in case."
- **Decisions go in ADRs.** Any significant or hard-to-reverse choice gets a new file in
  `docs/architecture/decisions/` (copy the latest as a template). Don't bury rationale in a
  commit message or a comment.
- **Keep it easy to read.** Short sentences, concrete examples, tables over walls of prose.

---

## 4. Adding a language (Definition of Done)

A language counts as **supported** only when **all** of these are true. This is also your
PR checklist:

- [ ] **Crate** — created `crates/compass-lang-<name>/` (start by copying
      `crates/compass-lang-template/`).
- [ ] **Detection** — declared `Detection { extensions, shebangs }`; the walker picks it up
      from the registry (don't edit the walker).
- [ ] **Parsing (`extract`)** — the tree-sitter grammar is wired in and symbols (functions,
      classes, etc.) are extracted per file.
- [ ] **Import resolution (`resolve`)** — the language's import/include mechanism resolves to
      real files using the `ResolutionContext`; unresolved targets are reported as
      `Diagnostic`s, not errors.
- [ ] **Graph output** — produces the **same** node/edge shape as every other language (no
      downstream special-casing).
- [ ] **Fixtures + tests** — a sample project + `insta` snapshot tests **inside the crate**
      at `crates/compass-lang-<name>/tests/`, asserting the expected nodes and edges.
- [ ] **No core changes** — implemented entirely behind the `Extractor` interface;
      `compass-core`, `compass-engine`, `compass-mcp`, and `compass-viz` untouched, and no other
      language crate referenced. The only shared edits allowed are in the **composition root**: the optional
      dependency + `lang-<name>` feature in `crates/compass-cli/Cargo.toml`, and one
      `register()` line in `register_all()`.
- [ ] **Docs** — added to the supported-languages list and the roadmap updated.

> If a change can't meet "Fixtures + tests" and "No core changes," the **extractor
> interface needs fixing first** — that's a higher priority than the language itself. Open
> an issue about the interface rather than working around it.

---

## 5. Code style & checks

- **Format:** `cargo fmt` (CI enforces it).
- **Lint:** `cargo clippy --all-targets -- -D warnings` must pass.
- **Test:** `cargo test`. For a single language, `cargo test -p compass-lang-<name>` compiles
  and tests just that one grammar.
- **No panics on input.** A malformed source file must produce a recorded error and let the
  index continue — never `unwrap()` on parsed content.
- **Errors:** `thiserror` in libraries, `anyhow` only at the CLI/server boundary.

---

## 6. Commits & pull requests

- **Conventional Commits** for messages: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`,
  `chore:`, etc. Scope by area when useful, e.g. `feat(lang-go): resolve relative imports`.
- **One logical change per commit;** keep history reviewable.
- **No AI/tool attribution in commit messages.** Do not add `Co-Authored-By` trailers or
  any automated-tool marks — project history stays clean.
- **PRs** should explain the *why*, link the issue, and (for a language) tick the §4
  checklist. Green CI (fmt + clippy + tests, including the isolated per-language build) is
  required to merge.

---

## 7. When in doubt, ask

Open an issue or a discussion. A question that clarifies a boundary or a rule is a
contribution in itself — and it often turns into an improvement to this document.
