# Compass + Claude Code

Two ways to give Claude Code the map. Use both — they're complementary (ADR-0006).

## 1. Pre-injection (recommended default) — a `UserPromptSubmit` hook

Before each prompt, inject the relevant slice of the map so the agent **reasons instead of
exploring**. A benchmark ([`docs/benchmarks/`](../../docs/benchmarks/README.md)) found this is
where the cost win actually is — making the agent *call* a tool adds turns that cancel the
savings; pre-loading the context doesn't.

**One-time setup in the repo you're working on:**

```sh
compass init        # writes .compass/ (the cached map — keeps injection fast) and .mcp.json
```

**Add this to that repo's `.claude/settings.json`** (or your global `~/.claude/settings.json`):

```json
{
  "hooks": {
    "UserPromptSubmit": [
      { "hooks": [ { "type": "command", "command": "compass context --hook" } ] }
    ]
  }
}
```

That's the whole hook — no shell scripting. `compass context --hook` reads Claude Code's
`UserPromptSubmit` payload from stdin (the prompt + cwd), ranks the most relevant files for
that prompt, and prints a compact map slice that Claude Code adds to the context. It's
cross-platform (the binary does the parsing), failure-safe (a problem prints nothing and never
blocks your prompt), and fast (it loads the `.compass/` cache rather than re-indexing — run
`compass watch` to keep that cache fresh, or it falls back to a one-off index).

Requirements: `compass` on your `PATH` (`cargo install --path crates/compass-cli`), or use an
absolute path in the `command`.

### Tune it

```sh
compass context --hook --max 8     # cap how many files are injected (default 12)
compass context . --query "add a retry to the HTTP client"      # try the ranking by hand
compass context . --file crates/foo/src/bar.rs                 # blast-radius around a file
```

## 2. MCP tools (deepening + any other host)

`compass init` also registers the MCP server (`compass serve`). Keep it: it's the **on-demand
deepening** path when the pre-injected slice didn't cover what the agent needs (`subgraph`,
`file_dependencies`, `shortest_path`, …), and it's the **only** integration that works across
non-Claude hosts (Cursor, etc.) — pre-injection hooks are per-host.

## 3. Slash command — `/compass …` inside a session

Run Compass without leaving your coding session. Copy [`compass.md`](../../.claude/commands/compass.md)
to your commands dir:

- **This project only:** it already lives at `.claude/commands/compass.md`.
- **Every project:** copy it to `~/.claude/commands/compass.md`.

Then (after restarting the session so the command is picked up):

```
/compass init                          # set up the current repo
/compass map                           # open the live visual map
/compass context --query "fix the ws reconnect"
/compass deps src/main.rs
```

It resolves the `compass` binary (PATH → repo `target/` → `cargo run`), runs long-lived
commands (`map`/`serve`/`watch`) in the background, and quick ones in the foreground.

## Why both (the hybrid)

Pre-injection front-loads the common case cheaply; MCP is the escape hatch when the prediction
misses and the portability story for other assistants. See
[ADR-0006](../../docs/architecture/decisions/0006-context-pre-injection.md).
