//! `compass init` / `compass install` — build the map and wire Compass into AI hosts
//! (Claude Code, Cursor, Codex): MCP config, the pre-injection hook, and the opt-in guard hook.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use compass_core::MapQuery;

use crate::{build_graph, clean_path};

/// `compass init` — make a project Compass-ready in one step: build (or refresh) the map,
/// and wire up MCP so an AI host auto-uses it. Idempotent — re-run anytime.
pub(crate) fn run_init(path: &Path) -> ExitCode {
    let Some(graph) = build_graph(path) else {
        return ExitCode::FAILURE;
    };
    if let Err(e) = compass_engine::cache::save(path, &graph) {
        eprintln!("compass: warning: could not write cache: {e:#}");
    }
    let overview = graph.overview();

    match write_mcp_config(path) {
        Ok(created) => println!(
            "compass: {} .mcp.json (registers the 'compass' MCP server for this repo)",
            if created { "created" } else { "updated" }
        ),
        Err(e) => eprintln!("compass: warning: could not write .mcp.json: {e:#}"),
    }

    println!(
        "compass: mapped {} files, {} symbols, {} import edges ({} diagnostics).",
        overview.file_count,
        overview.symbol_count,
        overview.import_edge_count,
        overview.diagnostic_count
    );
    println!("Ready — an MCP-capable assistant will use Compass in this repo.");
    println!("Tip: `compass watch` keeps the map fresh as you edit.");
    ExitCode::SUCCESS
}

/// Create or update a project `.mcp.json` so MCP hosts launch `compass serve` for this
/// repo, merging with any existing config (preserving other servers). Returns whether the
/// file was newly created.
fn write_mcp_config(path: &Path) -> anyhow::Result<bool> {
    use serde_json::{json, Value};

    let mcp_path = path.join(".mcp.json");
    let existed = mcp_path.exists();

    // Point at THIS binary and the absolute project path, so it works regardless of the
    // host's PATH or working directory.
    let exe = clean_path(&std::env::current_exe()?);
    let project = clean_path(&std::fs::canonicalize(path)?);
    let server = json!({ "command": exe, "args": ["serve", project] });

    let mut root: Value = if existed {
        serde_json::from_str(&std::fs::read_to_string(&mcp_path)?).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    if !root.is_object() {
        root = json!({});
    }
    let servers = root
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers
        .as_object_mut()
        .unwrap()
        .insert("compass".to_string(), server);

    std::fs::write(
        &mcp_path,
        format!("{}\n", serde_json::to_string_pretty(&root)?),
    )?;
    Ok(!existed)
}

/// Build a hook `command` string (`<exe> <subcommand>`) for a `.claude/settings.json` hook. The
/// host runs this through a shell, so an `exe` path containing spaces (e.g. `C:/Program Files/…`)
/// must be quoted or only the first word would be treated as the program.
fn hook_command(exe: &str, subcommand: &str) -> String {
    if exe.contains(' ') {
        format!("\"{exe}\" {subcommand}")
    } else {
        format!("{exe} {subcommand}")
    }
}

/// Marker heading for the Codex/AGENTS.md section — used both to write and to detect it.
const AGENTS_HEADING: &str = "## Compass";

/// The Codex/`AGENTS.md` guidance block appended by `install --codex`. Kept short and
/// host-agnostic: it just tells the agent a Compass map + MCP server is available and to
/// query it before grepping the whole tree.
const AGENTS_SECTION: &str = "## Compass

This repo has a [Compass](https://github.com/QuirijnPolinoco/compass) repo-map and an MCP
server wired in. Before grepping or reading across the whole tree, query the map first — it's
faster and cheaper:

- `overview` — files, languages, and the most-connected files at a glance.
- `subgraph` — the blast-radius around a file (what it imports and what imports it).
- `graph_stats` — hubs and community structure of the codebase.

Reach for these to orient before exploring, and fall back to reading files for the details.
";

/// `compass install [--claude] [--cursor] [--codex] [--all] [PATH]` — wire Compass into one
/// or more AI coding hosts by writing per-host config from embedded templates. Host-agnostic:
/// this only writes config files; the engine never depends on any host. No telemetry, no
/// network. Idempotent — safe to re-run; existing config is merged, never duplicated.
pub(crate) fn run_install(args: &[String]) -> ExitCode {
    let mut path = PathBuf::from(".");
    let mut claude = false;
    let mut cursor = false;
    let mut codex = false;
    let mut all = false;
    // The destructive-edit guard hook is strictly opt-in (a convenience, not a safety net), so
    // it is NEVER added by a bare `install`/`--all` — only when explicitly requested.
    let mut guard = false;

    for arg in args {
        match arg.as_str() {
            "--claude" => claude = true,
            "--cursor" => cursor = true,
            "--codex" => codex = true,
            "--all" => all = true,
            "--guard" => guard = true,
            other if other.starts_with('-') => {
                eprintln!("compass: unknown option `{other}` for `install`");
                return ExitCode::FAILURE;
            }
            other => path = PathBuf::from(other),
        }
    }

    // No selector at all → install the MCP/context wiring for every host (but still NOT the
    // opt-in guard). `--guard` counts as a selector, so `install --guard` adds only the guard.
    if all || !(claude || cursor || codex || guard) {
        claude = true;
        cursor = true;
        codex = true;
    }

    let mut wrote = false;
    if claude {
        match install_claude(&path) {
            Ok(()) => wrote = true,
            Err(e) => {
                eprintln!("compass: failed to install for Claude Code: {e:#}");
                return ExitCode::FAILURE;
            }
        }
    }
    if cursor {
        match install_cursor(&path) {
            Ok(()) => wrote = true,
            Err(e) => {
                eprintln!("compass: failed to install for Cursor: {e:#}");
                return ExitCode::FAILURE;
            }
        }
    }
    if codex {
        match install_codex(&path) {
            Ok(()) => wrote = true,
            Err(e) => {
                eprintln!("compass: failed to install for Codex: {e:#}");
                return ExitCode::FAILURE;
            }
        }
    }
    if guard {
        match write_claude_guard_settings(&path) {
            Ok(created) => {
                wrote = true;
                let settings = path.join(".claude").join("settings.json");
                println!(
                    "compass: {} {} (PreToolUse hook → `compass guard`)",
                    if created { "created" } else { "updated" },
                    clean_path(&settings)
                );
                println!(
                    "compass: the guard warns before editing a high-centrality file. It is a convenience, \
                     not a safety guarantee — it fails open (allows) on any doubt."
                );
                // The guard reads `.compass/` and does NOTHING (silently) without it. Tell the user
                // so `install --guard` alone doesn't give a false sense of security.
                if compass_engine::cache::exists(&path) {
                    println!(
                        "compass: it reads the cached map and never re-indexes — keep it fresh with \
                         `compass watch` (a stale map may miss a new hub or flag a former one)."
                    );
                } else {
                    println!(
                        "compass: NOTE — no Compass map found here yet, so the guard will do nothing. \
                         Run `compass init` (then `compass watch` to keep it fresh) to actually enable it."
                    );
                }
            }
            Err(e) => {
                eprintln!("compass: failed to install the guard hook: {e:#}");
                return ExitCode::FAILURE;
            }
        }
    }

    if wrote {
        println!("Done — an AI host configured here will use Compass.");
        println!("Tip: `compass watch` keeps the map fresh as you edit.");
    }
    ExitCode::SUCCESS
}

/// Wire up Claude Code: ensure `.mcp.json` exists (for the MCP deepening path), then merge a
/// `UserPromptSubmit` hook into `.claude/settings.json` that runs `compass context --hook`.
fn install_claude(path: &Path) -> anyhow::Result<()> {
    match write_mcp_config(path) {
        Ok(created) => println!(
            "compass: {} .mcp.json (registers the 'compass' MCP server for this repo)",
            if created { "created" } else { "updated" }
        ),
        Err(e) => eprintln!("compass: warning: could not write .mcp.json: {e:#}"),
    }

    let created = write_claude_settings(path)?;
    let settings = path.join(".claude").join("settings.json");
    println!(
        "compass: {} {} (UserPromptSubmit hook → `compass context --hook`)",
        if created { "created" } else { "updated" },
        clean_path(&settings)
    );
    Ok(())
}

/// Create or merge `.claude/settings.json`, adding a `UserPromptSubmit` hook that runs this
/// binary's absolute path with `context --hook`. Preserves any existing settings and hooks,
/// and is idempotent: an equivalent hook is never added twice. Returns whether the file was
/// newly created.
fn write_claude_settings(path: &Path) -> anyhow::Result<bool> {
    use serde_json::{json, Value};

    let dir = path.join(".claude");
    std::fs::create_dir_all(&dir)?;
    let settings_path = dir.join("settings.json");
    let existed = settings_path.exists();

    // Same absolute-binary approach as write_mcp_config, so the hook works regardless of PATH.
    let exe = clean_path(&std::env::current_exe()?);
    let command = hook_command(&exe, "context --hook");

    let mut root: Value = if existed {
        serde_json::from_str(&std::fs::read_to_string(&settings_path)?)
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    if !root.is_object() {
        root = json!({});
    }

    let hooks = root
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let submit = hooks
        .as_object_mut()
        .unwrap()
        .entry("UserPromptSubmit")
        .or_insert_with(|| json!([]));
    if !submit.is_array() {
        *submit = json!([]);
    }
    let entries = submit.as_array_mut().unwrap();

    // Idempotency: only the command tail (`… context --hook`) is matched, so a re-run with a
    // different binary path (or an already-present equivalent) is not duplicated.
    let already_present = entries.iter().any(|entry| {
        entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .is_some_and(|inner| {
                inner.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|c| c.contains("context --hook"))
                })
            })
    });

    if !already_present {
        entries.push(json!({
            "hooks": [ { "type": "command", "command": command } ]
        }));
    }

    std::fs::write(
        &settings_path,
        format!("{}\n", serde_json::to_string_pretty(&root)?),
    )?;
    Ok(!existed)
}

/// Create or merge `.claude/settings.json`, adding a `PreToolUse` hook that runs this binary's
/// `guard` subcommand on file-mutating tools. Opt-in only (`install --guard`): it warns before a
/// destructive edit to a high-centrality file. Preserves existing settings/hooks and is
/// idempotent — an equivalent guard hook is never added twice. Returns whether the file was newly
/// created. The matcher narrows the hook to edit tools so it never runs on reads/searches.
fn write_claude_guard_settings(path: &Path) -> anyhow::Result<bool> {
    use serde_json::{json, Value};

    let dir = path.join(".claude");
    std::fs::create_dir_all(&dir)?;
    let settings_path = dir.join("settings.json");
    let existed = settings_path.exists();

    let exe = clean_path(&std::env::current_exe()?);
    let command = hook_command(&exe, "guard");

    let mut root: Value = if existed {
        serde_json::from_str(&std::fs::read_to_string(&settings_path)?)
            .unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    if !root.is_object() {
        root = json!({});
    }

    let hooks = root
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| json!({}));
    if !hooks.is_object() {
        *hooks = json!({});
    }
    let pre = hooks
        .as_object_mut()
        .unwrap()
        .entry("PreToolUse")
        .or_insert_with(|| json!([]));
    if !pre.is_array() {
        *pre = json!([]);
    }
    let entries = pre.as_array_mut().unwrap();

    // Idempotency: a `… guard` command (the last token is exactly `guard`) is added at most once,
    // regardless of the binary path it's spelled with.
    let already_present = entries.iter().any(|entry| {
        entry
            .get("hooks")
            .and_then(|h| h.as_array())
            .is_some_and(|inner| {
                inner.iter().any(|h| {
                    h.get("command")
                        .and_then(|c| c.as_str())
                        .is_some_and(|c| c.split_whitespace().last() == Some("guard"))
                })
            })
    });

    if !already_present {
        entries.push(json!({
            "matcher": "Write|Edit|MultiEdit|NotebookEdit",
            "hooks": [ { "type": "command", "command": command } ]
        }));
    }

    std::fs::write(
        &settings_path,
        format!("{}\n", serde_json::to_string_pretty(&root)?),
    )?;
    Ok(!existed)
}

/// Wire up Cursor: create or merge `.cursor/mcp.json` with a "compass" server entry (same
/// command/args shape as `.mcp.json`), preserving any existing servers.
fn install_cursor(path: &Path) -> anyhow::Result<()> {
    use serde_json::{json, Value};

    let dir = path.join(".cursor");
    std::fs::create_dir_all(&dir)?;
    let mcp_path = dir.join("mcp.json");
    let existed = mcp_path.exists();

    let exe = clean_path(&std::env::current_exe()?);
    let project = clean_path(&std::fs::canonicalize(path)?);
    let server = json!({ "command": exe, "args": ["serve", project] });

    let mut root: Value = if existed {
        serde_json::from_str(&std::fs::read_to_string(&mcp_path)?).unwrap_or_else(|_| json!({}))
    } else {
        json!({})
    };
    if !root.is_object() {
        root = json!({});
    }
    let servers = root
        .as_object_mut()
        .unwrap()
        .entry("mcpServers")
        .or_insert_with(|| json!({}));
    if !servers.is_object() {
        *servers = json!({});
    }
    servers
        .as_object_mut()
        .unwrap()
        .insert("compass".to_string(), server);

    std::fs::write(
        &mcp_path,
        format!("{}\n", serde_json::to_string_pretty(&root)?),
    )?;
    println!(
        "compass: {} {} (registers the 'compass' MCP server for Cursor)",
        if existed { "updated" } else { "created" },
        clean_path(&mcp_path)
    );
    Ok(())
}

/// Wire up Codex/other AGENTS.md-aware hosts: create `AGENTS.md` with the Compass guidance, or
/// append a clearly-delimited `## Compass` section to an existing file. Idempotent: if a
/// `## Compass` heading is already present, nothing is changed.
fn install_codex(path: &Path) -> anyhow::Result<()> {
    let agents_path = path.join("AGENTS.md");
    let existing = std::fs::read_to_string(&agents_path).ok();

    if let Some(contents) = &existing {
        if contents.contains(AGENTS_HEADING) {
            println!(
                "compass: {} already has a `{AGENTS_HEADING}` section (left unchanged)",
                clean_path(&agents_path)
            );
            return Ok(());
        }
    }

    match existing {
        Some(contents) => {
            let mut out = contents;
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push('\n');
            out.push_str(AGENTS_SECTION);
            std::fs::write(&agents_path, out)?;
            println!(
                "compass: updated {} (appended a `{AGENTS_HEADING}` section)",
                clean_path(&agents_path)
            );
        }
        None => {
            std::fs::write(&agents_path, AGENTS_SECTION)?;
            println!("compass: created {}", clean_path(&agents_path));
        }
    }
    Ok(())
}
