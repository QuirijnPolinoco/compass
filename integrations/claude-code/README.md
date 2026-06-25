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

## 1b. Guard (opt-in) — a `PreToolUse` hub-edit confirmation

A **convenience, not a safety guarantee.** When wired as a `PreToolUse` hook, `compass guard`
reads the pending tool call from stdin and, for a destructive edit (`Write`/`Edit`/`MultiEdit`/
`NotebookEdit`) to a **high-centrality file** (a hub / heavily-depended-on file in the cached
map), asks you to confirm before the edit runs. Everything else is allowed. It is engineered to
**fail open**: on any uncertainty — bad input, an unknown tool, a path it can't map into the repo,
no cache, or a file that isn't in the map — it allows silently and never blocks or panics.

It is **not** wired by `compass install`/`--all`. Add it explicitly:

```sh
compass init                   # FIRST: build the .compass map the guard reads (or it does nothing)
compass install --guard        # adds ONLY the PreToolUse guard hook to .claude/settings.json
```

> The guard reads the cached `.compass/` map and **does nothing** without it — `install --guard`
> only writes the hook, so run `compass init` first (it warns if no map is present). It launches
> from the hook payload's `cwd` and searches upward for `.compass`, so running `claude` from a
> subdirectory of the repo still works.

That writes:

```json
{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Write|Edit|MultiEdit|NotebookEdit",
        "hooks": [ { "type": "command", "command": "compass guard" } ]
      }
    ]
  }
}
```

`compass guard` loads the `.compass/` cache (it never re-indexes, so it's instant on every tool
call). A file counts as high-centrality if it's a structural **hub** (bridges multiple parts of the
codebase) **or** it is **depended on by** at/above a threshold number of files — its in-degree, i.e.
how many files import it (default: the top decile, with a small floor for tiny repos). The metric is
deliberately *in*-degree, not in + out: the blast radius of editing a file is the set of files that
depend on it, so a pure aggregator/entrypoint (a `main.go`/`mod.rs` that imports many but is
imported by none) is **not** flagged.

**Keep the map fresh.** The guard trusts whatever the cache says and never re-indexes, so against a
stale map it may miss a newly-central file or flag a file that's no longer central. Run
`compass watch` (or re-run `compass init`) to keep `.compass/` current.

**What it does *not* see.** The guard only fires for the `Write`/`Edit`/`MultiEdit`/`NotebookEdit`
tools. Edits made through the **Bash** tool (`sed -i`, `>` redirection, codegen scripts, `mv`/`cp`)
are invisible to it. It's a convenience, not a containment boundary.

### Tune it

Two environment variables tune the guard. Because it runs as a Claude Code **hook**, it sees the
environment Claude Code launches hooks in — **not** a variable you just typed into your shell. Set
them so the hook process actually inherits them: either a persistent/global export (e.g. in your
shell profile), or, if your host supports it, an `env` block on the hook in `.claude/settings.json`.

```sh
COMPASS_GUARD_MIN_DEGREE=8   # override the in-degree threshold (how many files import it)
COMPASS_GUARD_BLOCK=1        # escalate from a non-blocking "ask" to a hard "deny" (opt-in)
```

By default the guard emits a non-blocking **ask** (you confirm) and exits 0, so you stay in
control. It asks *every* time you edit a hub (Claude Code's own prompt offers "don't ask again" to
quiet repeats); it deliberately does not silently suppress a repeat, because the hook can't see how
you answered and suppressing would let an edit through right after you declined one. Note that
declining an ask does block *that one* edit, and a fully non-interactive run (`claude -p`, CI) may
have no one to answer — so the default is not "literally never blocks", it's "never hard-*denies*".

`COMPASS_GUARD_BLOCK=1` opts into a hard `deny` instead; leave it unset for the safe default. In
block mode a given hub is denied at most **once per session** and allowed thereafter — so a hard
block can never permanently wedge you off a file.

### Turning it off

There is no `uninstall` command yet. To remove the guard, delete the `PreToolUse` entry whose
`command` ends in `guard` from `.claude/settings.json` (the matcher is
`Write|Edit|MultiEdit|NotebookEdit`). Removing the whole `PreToolUse` array works too if it's the
only hook there.

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
