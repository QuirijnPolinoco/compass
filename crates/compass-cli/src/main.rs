//! `compass` — CLI entrypoint and composition root.
//!
//! Builds the language registry via the explicit [`registry::register_all`] (ADR-0003),
//! runs the engine, and renders results (or serves them over MCP).

mod registry;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use compass_core::{ContextPack, ContextRequest, EdgeKind, Graph, MapQuery, NodeKind};
use serde::{Deserialize, Serialize};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("overview");
    let path = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    match command {
        "init" => run_init(&path),
        "install" => run_install(&args[1..]),
        "overview" => run_overview(&path),
        "languages" => run_languages(),
        "deps" => run_deps(&path, args.get(2).map(String::as_str)),
        "broken" => run_broken(&path),
        "watch" => run_watch(&path),
        "map" => run_map(&args[1..]),
        "context" => run_context(&args[1..]),
        "guard" => run_guard(&path),
        "serve" => run_serve(&path),
        "help" | "-h" | "--help" => {
            print_help();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("compass: unknown command `{other}`\n");
            print_help();
            ExitCode::FAILURE
        }
    }
}

/// Index `path` with the compiled-in extractors, or print an error and return `None`.
///
/// Incremental by default: reuses the on-disk per-file extraction cache (`.compass/`) so files
/// whose `(mtime, size)` are unchanged are not re-read or re-parsed — the dominant cost on a
/// large repo. The refreshed cache is written back (best-effort) so the next run stays fast.
/// Delete `.compass/` (or it auto-invalidates on a format bump) to force a full re-index.
fn build_graph(path: &Path) -> Option<Graph> {
    let registry = registry::register_all();
    let prev = compass_engine::cache::load_extractions(path);
    match compass_engine::index_incremental(path, &registry, prev.as_ref()) {
        Ok((graph, extractions)) => {
            let _ = compass_engine::cache::save_extractions(path, &extractions);
            Some(graph)
        }
        Err(e) => {
            eprintln!("compass: failed to index {}: {e:#}", path.display());
            None
        }
    }
}

/// `compass init` — make a project Compass-ready in one step: build (or refresh) the map,
/// and wire up MCP so an AI host auto-uses it. Idempotent — re-run anytime.
fn run_init(path: &Path) -> ExitCode {
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

/// Absolute path as a clean forward-slash string (strips Windows `\\?\` verbatim prefix).
fn clean_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).replace('\\', "/")
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
fn run_install(args: &[String]) -> ExitCode {
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

fn run_overview(path: &Path) -> ExitCode {
    let Some(graph) = build_graph(path) else {
        return ExitCode::FAILURE;
    };
    if let Err(e) = compass_engine::cache::save(path, &graph) {
        eprintln!("compass: warning: could not write cache: {e:#}");
    }

    let overview = graph.overview();
    println!("Compass overview — {}", path.display());
    println!("  files:        {}", overview.file_count);
    println!("  symbols:      {}", overview.symbol_count);
    println!("  import edges:  {}", overview.import_edge_count);
    println!("  diagnostics:  {}", overview.diagnostic_count);
    if !overview.languages.is_empty() {
        println!("  languages:");
        for stat in &overview.languages {
            println!(
                "    {:<12} {} file(s)",
                stat.language.as_str(),
                stat.file_count
            );
        }
    }
    if !overview.most_connected.is_empty() {
        println!("  most connected:");
        for c in &overview.most_connected {
            println!("    {:>3}  {}", c.connections, c.file);
        }
    }
    ExitCode::SUCCESS
}

fn run_deps(path: &Path, file: Option<&str>) -> ExitCode {
    let Some(file) = file else {
        eprintln!("usage: compass deps <PATH> <FILE>   (FILE is repo-relative, e.g. src/main.go)");
        return ExitCode::FAILURE;
    };
    let Some(graph) = build_graph(path) else {
        return ExitCode::FAILURE;
    };
    match graph.file_dependencies(file) {
        Some(deps) => {
            println!("{}", deps.file);
            println!("  depends on ({}):", deps.dependencies.len());
            for dep in &deps.dependencies {
                println!("    -> {dep}");
            }
            println!("  depended on by ({}):", deps.dependents.len());
            for dep in &deps.dependents {
                println!("    <- {dep}");
            }
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("compass: `{file}` is not in the map (use a repo-relative path)");
            ExitCode::FAILURE
        }
    }
}

fn run_broken(path: &Path) -> ExitCode {
    let Some(graph) = build_graph(path) else {
        return ExitCode::FAILURE;
    };
    let broken = graph.broken_imports();
    if broken.is_empty() {
        println!("No broken imports.");
    } else {
        println!("Broken imports ({}):", broken.len());
        for b in &broken {
            println!("  {} — {}", b.file, b.message);
        }
    }
    ExitCode::SUCCESS
}

fn run_languages() -> ExitCode {
    let registry = registry::register_all();
    let ids = registry.language_ids();
    println!("Supported languages ({}):", ids.len());
    for id in ids {
        println!("  - {id}");
    }
    ExitCode::SUCCESS
}

fn run_watch(path: &Path) -> ExitCode {
    let Some(graph) = build_graph(path) else {
        return ExitCode::FAILURE;
    };
    let _ = compass_engine::cache::save(path, &graph);
    let ov = graph.overview();
    println!(
        "Watching {} — {} files, {} symbols, {} import edges, {} diagnostics",
        path.display(),
        ov.file_count,
        ov.symbol_count,
        ov.import_edge_count,
        ov.diagnostic_count
    );
    println!("Editing files re-maps automatically. Press Ctrl+C to stop.");

    let result =
        compass_engine::watch::watch(path, std::time::Duration::from_millis(500), |paths| {
            if let Some(graph) = build_graph(path) {
                let _ = compass_engine::cache::save(path, &graph);
                let ov = graph.overview();
                println!(
                    "~ {} change(s) -> {} files, {} symbols, {} import edges, {} diagnostics",
                    paths.len(),
                    ov.file_count,
                    ov.symbol_count,
                    ov.import_edge_count,
                    ov.diagnostic_count
                );
            }
        });
    if let Err(e) = result {
        eprintln!("compass: watch error: {e:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// `compass map` — serve the interactive visual map on localhost and keep it live as you
/// edit (ADR-0005). `--snapshot` instead writes a self-contained `.compass/map.html` and
/// exits. The viz server is the composition point that wires the engine into `compass-viz`
/// and republishes a fresh map on every watch event.
fn run_map(args: &[String]) -> ExitCode {
    let mut path = PathBuf::from(".");
    let mut port: Option<u16> = None;
    let mut open = true;
    let mut snapshot = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--no-open" => open = false,
            "--snapshot" => snapshot = true,
            "--port" => match iter.next().and_then(|p| p.parse::<u16>().ok()) {
                Some(p) => port = Some(p),
                None => {
                    eprintln!("compass: --port needs a number, e.g. `--port 62049`");
                    return ExitCode::FAILURE;
                }
            },
            other if other.starts_with("--port=") => {
                match other["--port=".len()..].parse::<u16>() {
                    Ok(p) => port = Some(p),
                    Err(_) => {
                        eprintln!("compass: --port needs a number, e.g. `--port=62049`");
                        return ExitCode::FAILURE;
                    }
                }
            }
            other if other.starts_with('-') => {
                eprintln!("compass: unknown option `{other}` for `map`");
                return ExitCode::FAILURE;
            }
            other => path = PathBuf::from(other),
        }
    }

    let Some(graph) = build_graph(&path) else {
        return ExitCode::FAILURE;
    };
    if let Err(e) = compass_engine::cache::save(&path, &graph) {
        eprintln!("compass: warning: could not write cache: {e:#}");
    }

    if snapshot {
        let query: compass_viz::Query = Arc::new(graph);
        let out = path.join(".compass").join("map.html");
        match std::fs::write(&out, compass_viz::snapshot_html(&query)) {
            Ok(()) => {
                println!("compass: wrote {} (open it in a browser).", out.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("compass: could not write {}: {e:#}", out.display());
                ExitCode::FAILURE
            }
        }
    } else {
        let state = compass_viz::MapState::new(Arc::new(graph), path.clone());
        let server = match compass_viz::bind(port) {
            Ok(server) => server,
            Err(e) => {
                eprintln!("compass: could not start the map server: {e}");
                return ExitCode::FAILURE;
            }
        };
        let url = server.url();
        println!("compass: serving the map at {url}");
        println!("Editing files updates it live. Press Ctrl+C to stop.");

        let server_state = Arc::clone(&state);
        std::thread::spawn(move || server.run(server_state));
        if open {
            open_browser(&url);
        }

        let watch_path = path.clone();
        let watch_state = Arc::clone(&state);
        let result =
            compass_engine::watch::watch(&path, std::time::Duration::from_millis(500), move |_| {
                if let Some(graph) = build_graph(&watch_path) {
                    let _ = compass_engine::cache::save(&watch_path, &graph);
                    watch_state.publish(Arc::new(graph));
                }
            });
        if let Err(e) = result {
            eprintln!("compass: watch error: {e:#}");
            return ExitCode::FAILURE;
        }
        ExitCode::SUCCESS
    }
}

/// Best-effort: open `url` in the default browser. The URL is always printed too, so a
/// failure here is non-fatal.
fn open_browser(url: &str) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("cmd")
        .args(["/C", "start", "", url])
        .spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();

    let _ = result;
}

/// `compass context` — print a token-bounded context pack for **pre-injection** into an AI
/// prompt (ADR-0006): a structural summary + the most relevant files. `--query` ranks by the
/// task text; `--file` seeds the blast-radius around files being worked on; otherwise the
/// most-connected files are returned.
fn run_context(args: &[String]) -> ExitCode {
    let mut path = PathBuf::from(".");
    let mut query: Option<String> = None;
    let mut seeds: Vec<String> = Vec::new();
    let mut max_files = 12usize;
    let mut hook = false;
    let mut fresh = false;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            // Read the prompt (and cwd) from a Claude Code UserPromptSubmit JSON on stdin,
            // so the hook needs no shell scripting: `compass context --hook`.
            "--hook" => hook = true,
            // Force a fresh index instead of loading the `.compass/` cache.
            "--fresh" => fresh = true,
            "--query" | "-q" => query = iter.next().cloned(),
            "--file" | "-f" => {
                if let Some(f) = iter.next() {
                    seeds.push(f.clone());
                }
            }
            "--max" => {
                max_files = iter
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(max_files)
            }
            other if other.starts_with("--query=") => {
                query = Some(other["--query=".len()..].to_string())
            }
            other if other.starts_with("--max=") => {
                if let Ok(n) = other["--max=".len()..].parse() {
                    max_files = n;
                }
            }
            other if other.starts_with('-') => {
                eprintln!("compass: unknown option `{other}` for `context`");
                return ExitCode::FAILURE;
            }
            other => path = PathBuf::from(other),
        }
    }

    let mut session_id: Option<String> = None;
    if hook {
        // UserPromptSubmit payload: { "prompt", "cwd", "session_id", ... }. Pull the prompt
        // (as the query), the cwd (if no PATH given), and the session id (for the session
        // graph). Anything missing → just skip.
        use std::io::Read as _;
        let mut buf = String::new();
        if std::io::stdin().read_to_string(&mut buf).is_ok() {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&buf) {
                if query.is_none() {
                    query = v.get("prompt").and_then(|p| p.as_str()).map(String::from);
                }
                if path.as_path() == Path::new(".") {
                    if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                        path = PathBuf::from(cwd);
                    }
                }
                session_id = v
                    .get("session_id")
                    .and_then(|s| s.as_str())
                    .map(String::from);
            }
        }
    }

    // Prefer the cached graph so per-prompt injection is fast (a full re-index every prompt
    // would tax a large repo). `--fresh` forces re-indexing; `compass init`/`watch` keep the
    // cache current. In hook mode a failure must never block the user's prompt — exit 0 silent.
    let graph = if fresh {
        None
    } else {
        compass_engine::cache::load(&path)
    }
    .or_else(|| build_graph(&path));
    let Some(graph) = graph else {
        return if hook {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    };
    let mut pack = graph.context(&ContextRequest {
        query,
        seeds,
        max_files,
    });

    // Session graph (ADR-0006 follow-up): within one editor session, don't re-inject files
    // already shown — they're still in the conversation. Keep only files new to this session,
    // then remember them. If nothing is new, inject nothing (don't spend tokens repeating).
    //
    // While we're here, record an honest token-savings estimate for the local dashboard
    // (`compass map` → `/tokens`). Token counts are ESTIMATES (rendered chars / 4), never
    // exact; the measurable story is `est_tokens_saved`: tokens NOT re-injected because the
    // files were already shown this session.
    if let Some(sid) = session_id.filter(|_| hook) {
        // Render the full selection first; the markdown it sheds after de-dup is what we did
        // not re-inject for already-seen files. The shared header cancels in the difference.
        let total_files = pack.files.len();
        let full_len = render_context_markdown(&path, &pack).len();

        let mut seen = load_session_seen(&path, &sid);
        pack.files.retain(|f| !seen.contains(&f.path));
        let files_injected = pack.files.len();
        let files_deduped = total_files - files_injected;

        if pack.files.is_empty() {
            // Everything we'd have shown is already in the session — inject nothing, but still
            // log what de-dup saved (the whole selection's estimated tokens).
            log_session_tokens(
                &path,
                &sid,
                TokenEvent {
                    at: unix_secs(),
                    files_injected: 0,
                    files_deduped,
                    est_tokens_injected: 0,
                    est_tokens_saved: (full_len / 4) as u64,
                },
            );
            return ExitCode::SUCCESS;
        }

        let injected_len = render_context_markdown(&path, &pack).len();
        log_session_tokens(
            &path,
            &sid,
            TokenEvent {
                at: unix_secs(),
                files_injected,
                files_deduped,
                est_tokens_injected: (injected_len / 4) as u64,
                est_tokens_saved: (full_len.saturating_sub(injected_len) / 4) as u64,
            },
        );

        for f in &pack.files {
            seen.push(f.path.clone());
        }
        save_session_seen(&path, &sid, &seen);
    }

    print!("{}", render_context_markdown(&path, &pack));
    ExitCode::SUCCESS
}

/// A repo's `.compass/sessions/` directory, where per-session state lives.
fn sessions_dir(repo: &Path) -> PathBuf {
    repo.join(".compass").join("sessions")
}

/// Filename-safe form of a host-generated session id (UUIDs); keep only safe chars defensively.
/// The seen-list (`<id>.json`) and the token log (`<id>.tokens.json`) share this same key.
fn safe_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Path of a session's "already-injected files" list under the repo's `.compass/sessions/`.
fn session_seen_path(repo: &Path, session_id: &str) -> PathBuf {
    sessions_dir(repo).join(format!("{}.json", safe_session_id(session_id)))
}

/// Files already injected this session (most-recent last), or empty if none/unreadable.
fn load_session_seen(repo: &Path, session_id: &str) -> Vec<String> {
    std::fs::read_to_string(session_seen_path(repo, session_id))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

/// Persist the session's injected-files list, capped to the most recent 1000 (best-effort).
fn save_session_seen(repo: &Path, session_id: &str, seen: &[String]) {
    let path = session_seen_path(repo, session_id);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let start = seen.len().saturating_sub(1000);
    if let Ok(json) = serde_json::to_string(&seen[start..]) {
        let _ = std::fs::write(path, json);
    }
}

/// One pre-injection event for the local token-savings dashboard. All token counts are
/// ESTIMATES (rendered markdown length / 4), never exact — the dashboard labels them so. The
/// honest, measurable number is `est_tokens_saved`: tokens NOT re-injected this session because
/// the files were already shown.
#[derive(Serialize, Deserialize)]
struct TokenEvent {
    /// Unix seconds when the event was recorded.
    at: u64,
    /// Files injected this prompt (after session de-dup).
    files_injected: usize,
    /// Files dropped because they were already injected earlier this session.
    files_deduped: usize,
    /// Estimated tokens injected (chars/4 of the injected markdown).
    est_tokens_injected: u64,
    /// Estimated tokens NOT re-injected thanks to de-dup (chars/4 of the dropped files).
    est_tokens_saved: u64,
}

/// Path of a session's token-savings log (a `Vec<TokenEvent>`) under `.compass/sessions/`.
fn token_log_path(repo: &Path, session_id: &str) -> PathBuf {
    sessions_dir(repo).join(format!("{}.tokens.json", safe_session_id(session_id)))
}

/// Append a token event to the session's log, capped to the most recent 500 (best-effort).
/// Like [`save_session_seen`], this must never block or fail the hook — IO/serialize errors are
/// ignored, and a malformed existing log is simply overwritten.
fn log_session_tokens(repo: &Path, session_id: &str, event: TokenEvent) {
    let path = token_log_path(repo, session_id);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut events: Vec<TokenEvent> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    events.push(event);
    let start = events.len().saturating_sub(500);
    if let Ok(json) = serde_json::to_string(&events[start..]) {
        let _ = std::fs::write(path, json);
    }
}

/// Current unix time in whole seconds (0 if the clock predates the epoch — never panics).
fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Render a context pack as a compact markdown block suitable for prompt injection.
fn render_context_markdown(path: &Path, pack: &ContextPack) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let langs = pack
        .languages
        .iter()
        .map(|l| format!("{} {}", l.language.as_str(), l.file_count))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        out,
        "# Compass map — {} ({} files; {})",
        path.display(),
        pack.file_count,
        langs
    );
    if !pack.most_connected.is_empty() {
        let mc = pack
            .most_connected
            .iter()
            .take(5)
            .map(|c| c.file.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "Most-connected: {mc}");
    }
    let _ = writeln!(out, "\nRelevant files (selected by {}):", pack.selected_by);
    for f in &pack.files {
        let lang = f.language.as_deref().unwrap_or("?");
        let mut line = format!("- {} [{lang}]", f.path);
        if !f.symbols.is_empty() {
            let _ = write!(line, " — symbols: {}", f.symbols.join(", "));
        }
        if !f.depends_on.is_empty() {
            let _ = write!(line, " — imports: {}", f.depends_on.join(", "));
        }
        if !f.dependents.is_empty() {
            let _ = write!(line, " — imported by: {}", f.dependents.join(", "));
        }
        let _ = writeln!(out, "{line}");
    }
    out
}

fn run_serve(path: &Path) -> ExitCode {
    let Some(graph) = build_graph(path) else {
        return ExitCode::FAILURE;
    };
    let query: std::sync::Arc<dyn MapQuery + Send + Sync> = std::sync::Arc::new(graph);
    if let Err(e) = compass_mcp::serve_stdio(query) {
        eprintln!("compass: MCP server error: {e:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Largest PreToolUse payload the guard will read from stdin (8 MiB). Real payloads are tiny; the
/// cap keeps fail-open total — a pathological multi-GB stdin can't OOM the process into a non-zero
/// exit a host might read as a block.
const GUARD_STDIN_CAP: u64 = 8 * 1024 * 1024;

/// `compass guard [PATH]` — a Claude Code **PreToolUse** hook (opt-in via `install --guard`). It
/// reads the pending tool call from stdin and, for a destructive edit to a high-centrality file (a
/// hub / heavily-depended-on file in the cached map), emits a non-blocking "ask the user to
/// confirm" decision; everything else is allowed.
///
/// This is a **convenience, not a safety guarantee**. It is engineered to FAIL OPEN: on any doubt
/// — bad input, an unknown tool, an unmappable path, no cache, a file not in the map — it allows
/// silently (exit 0, no output) and it NEVER panics or hard-blocks. The default decision is `ask`
/// (the user confirms); `COMPASS_GUARD_BLOCK=1` opts into a hard `deny` instead.
fn run_guard(path: &Path) -> ExitCode {
    use std::io::Read as _;

    // Read the PreToolUse JSON from stdin (capped — see GUARD_STDIN_CAP). Unreadable → allow.
    let mut buf = String::new();
    if std::io::stdin()
        .take(GUARD_STDIN_CAP)
        .read_to_string(&mut buf)
        .is_err()
    {
        return ExitCode::SUCCESS;
    }
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(&buf) else {
        return ExitCode::SUCCESS;
    };

    // Only act on file-mutating tools; everything else (Read, Bash, Grep, …) is allowed silently.
    let tool_name = payload
        .get("tool_name")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if !matches!(tool_name, "Write" | "Edit" | "MultiEdit" | "NotebookEdit") {
        return ExitCode::SUCCESS;
    }

    // The target file path; absent → allow silently.
    let Some(target) = guard_target_path(&payload) else {
        return ExitCode::SUCCESS;
    };

    // Resolve the repo root. An explicit PATH argument wins (the user/host told us exactly where);
    // otherwise fall back to the PreToolUse payload's `cwd` (where Claude Code was launched) and
    // search upward for a `.compass` map the way git finds `.git`, so running `claude` from a
    // SUBDIRECTORY of the repo still resolves the root instead of silently no-opping.
    let repo_root: PathBuf = if path != Path::new(".") {
        path.to_path_buf()
    } else {
        let base = payload
            .get("cwd")
            .and_then(|c| c.as_str())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        find_compass_root(&base).unwrap_or(base)
    };

    // Map the target to a repo-relative, forward-slash path; outside the repo / unmappable → allow.
    let Some(rel) = guard_repo_relative(&repo_root, &target) else {
        return ExitCode::SUCCESS;
    };

    // Load the CACHED graph only — never re-index (the hook runs on every tool call, must be
    // instant). No cache → allow silently. NOTE: the guard trusts whatever the cache says; it does
    // not check freshness, so keep it current with `compass watch`/`compass init` (a stale map may
    // miss a new hub or flag a former one).
    let Some(graph) = compass_engine::cache::load(&repo_root) else {
        return ExitCode::SUCCESS;
    };

    // Assess centrality from the cached graph. Not in the map → allow silently.
    let Some(assessment) = assess_centrality(&graph, &rel) else {
        return ExitCode::SUCCESS;
    };
    if !assessment.high_centrality {
        return ExitCode::SUCCESS;
    }

    let block = guard_block_enabled();

    // De-dup ONLY in hard-block mode: deny a given hub at most once per session, then allow — so a
    // hard `deny` can never permanently wedge you off a file. The default `ask` path intentionally
    // does NOT de-dup: the hook is stateless and cannot observe the user's answer, so suppressing a
    // repeat would silently ALLOW the next edit even after the user just DECLINED. Asking every time
    // keeps the user in control (Claude Code's own prompt offers "don't ask again" to quiet repeats).
    if block {
        if let Some(sid) = payload.get("session_id").and_then(|s| s.as_str()) {
            let mut warned = load_guard_warned(&repo_root, sid);
            if warned.iter().any(|w| w == &rel) {
                return ExitCode::SUCCESS; // already denied once this session → allow silently
            }
            warned.push(rel.clone());
            save_guard_warned(&repo_root, sid, &warned);
        }
    }

    // Emit the decision. Default `ask` (the user confirms); opt-in `deny` via COMPASS_GUARD_BLOCK.
    let decision = if block { "deny" } else { "ask" };
    let reason = guard_reason(&rel, &assessment);
    let out = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": decision,
            "permissionDecisionReason": reason,
        }
    });
    // Compact JSON on stdout + exit 0 is the contract for a structured PreToolUse decision.
    println!("{}", serde_json::to_string(&out).unwrap_or_default());
    ExitCode::SUCCESS
}

/// Walk up from `start` (inclusive) looking for a directory that holds a `.compass` map, the way
/// git finds `.git`. Returns the first such ancestor, or `None` if none up to the filesystem root.
/// Lets the guard resolve the repo root when Claude Code is launched from a subdirectory.
fn find_compass_root(start: &Path) -> Option<PathBuf> {
    let start_abs = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    let mut cur: Option<&Path> = Some(start_abs.as_path());
    while let Some(dir) = cur {
        if compass_engine::cache::exists(dir) {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// Pull the file path a mutating tool will touch out of its `tool_input`. Handles `file_path`
/// (Write/Edit/MultiEdit), `notebook_path` (NotebookEdit), and the `edits[].file_path` shape.
/// Returns `None` (→ allow silently) if no path is present.
fn guard_target_path(payload: &serde_json::Value) -> Option<String> {
    let input = payload.get("tool_input")?;
    if let Some(p) = input.get("file_path").and_then(|v| v.as_str()) {
        return Some(p.to_string());
    }
    if let Some(p) = input.get("notebook_path").and_then(|v| v.as_str()) {
        return Some(p.to_string());
    }
    if let Some(edits) = input.get("edits").and_then(|v| v.as_array()) {
        for e in edits {
            if let Some(p) = e.get("file_path").and_then(|v| v.as_str()) {
                return Some(p.to_string());
            }
        }
    }
    None
}

/// Map a tool's target path to the repo-relative, forward-slash key the map uses, or `None` if it
/// can't be confidently mapped into `repo_root` (→ allow silently). Absolute paths are resolved
/// against the canonical repo root; relative paths are joined onto it. The boundary must fall on a
/// path separator, so a sibling dir sharing a name prefix is never mistaken for being inside.
fn guard_repo_relative(repo_root: &Path, target: &str) -> Option<String> {
    let repo_abs = std::fs::canonicalize(repo_root).ok()?;
    let target_path = Path::new(target);
    let target_abs = if target_path.is_absolute() {
        // Existing files (the edit case we care about) canonicalize; a not-yet-created file falls
        // back to its given path — and if that can't be matched below, we just fail open.
        std::fs::canonicalize(target_path).unwrap_or_else(|_| target_path.to_path_buf())
    } else {
        repo_abs.join(target_path)
    };

    let repo_s = clean_path(&repo_abs);
    let target_s = clean_path(&target_abs);
    let rest = target_s.strip_prefix(&repo_s)?;
    if !rest.starts_with('/') {
        return None; // equal paths, or a `repo`-vs-`repo-other` prefix collision
    }
    let rel = rest.trim_start_matches('/');
    (!rel.is_empty()).then(|| rel.to_string())
}

/// What the guard learned about a target file's place in the map.
struct GuardAssessment {
    /// Whether the file crosses the high-centrality bar (hub OR dependents ≥ threshold).
    high_centrality: bool,
    /// A community-bridging hub (the existing Louvain hub flag).
    is_hub: bool,
    /// Files that import this one (its in-degree) — the blast radius of editing it, and the
    /// selection metric for the "heavily depended on" branch.
    dependents: usize,
    /// Distinct communities its neighbors span, if it's a hub.
    communities_bridged: usize,
}

/// Assess how central `rel` is in the cached graph, or `None` if it isn't in the map. A file is
/// high-centrality if it is a structural hub OR enough files import it (in-degree at/above the
/// threshold, see [`guard_degree_threshold`]).
///
/// The selection metric is **in-degree** (how many files import this one), NOT in+out degree: the
/// blast radius of editing a file is the set of files that *depend on* it. Selecting on in+out
/// would flag pure aggregators/entrypoints (a `main.go`/`mod.rs` that imports many packages but is
/// imported by none has zero blast radius) and make the "heavily depended on" reason contradict
/// itself ("0 file(s) import it (import degree 6)").
fn assess_centrality(graph: &Graph, rel: &str) -> Option<GuardAssessment> {
    let view = graph.graph_view(false);

    // In-degree per file: graph_view import edges run importer(source) → imported(target), so a
    // file's in-degree (its dependents) is the number of import edges that point AT it.
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for e in &view.edges {
        if e.kind == EdgeKind::Import {
            *in_degree.entry(e.target.as_str()).or_insert(0) += 1;
        }
    }

    // One pass over the file nodes: collect every file's in-degree (for the threshold) and find the
    // target's own in-degree + hub flag. On case-insensitive filesystems (Windows/macOS) fall back
    // to a case-insensitive match so a casing difference doesn't silently skip a hub.
    let ci = cfg!(any(windows, target_os = "macos"));
    let mut dependent_degrees: Vec<usize> = Vec::with_capacity(view.nodes.len());
    let mut exact: Option<(usize, bool)> = None;
    let mut ci_match: Option<(usize, bool)> = None;
    for n in &view.nodes {
        if n.kind != NodeKind::File {
            continue;
        }
        let d = in_degree.get(n.path.as_str()).copied().unwrap_or(0);
        dependent_degrees.push(d);
        if n.path == rel {
            exact = Some((d, n.is_hub));
        } else if ci && n.path.eq_ignore_ascii_case(rel) {
            ci_match = Some((d, n.is_hub));
        }
    }
    let (dependents, is_hub) = exact.or(ci_match)?; // not in the map → None → allow silently

    let threshold = guard_degree_threshold(&dependent_degrees);
    let high_centrality = is_hub || dependents >= threshold;

    // Only spend the extra pass (communities) once we know we'll warn about a hub.
    let communities_bridged = if high_centrality && is_hub {
        graph
            .hubs()
            .into_iter()
            .find(|h| h.file == rel)
            .map(|h| h.communities_bridged)
            .unwrap_or(0)
    } else {
        0
    };

    Some(GuardAssessment {
        high_centrality,
        is_hub,
        dependents,
        communities_bridged,
    })
}

/// In-degree (number of files that import a file) at/above which it counts as high-centrality.
/// Defaults to the top decile of all files' in-degree, floored at a small absolute
/// ([`GUARD_DEGREE_FLOOR`]) so a tiny repo doesn't flag a barely-depended-on file. Override with
/// `COMPASS_GUARD_MIN_DEGREE`.
fn guard_degree_threshold(degrees: &[usize]) -> usize {
    if let Ok(raw) = std::env::var("COMPASS_GUARD_MIN_DEGREE") {
        if let Ok(n) = raw.trim().parse::<usize>() {
            return n.max(1);
        }
    }
    if degrees.is_empty() {
        return usize::MAX; // nothing to compare against → never trips on degree (hubs still do)
    }
    let mut sorted = degrees.to_vec();
    sorted.sort_unstable();
    let idx = ((sorted.len() * 9) / 10).min(sorted.len() - 1); // 90th percentile
    sorted[idx].max(GUARD_DEGREE_FLOOR)
}

/// Smallest in-degree (number of importers) the default threshold will ever flag, so a tiny repo's
/// loosely-depended-on files aren't warned about. Genuine community-bridging hubs are flagged
/// regardless of how many files import them.
const GUARD_DEGREE_FLOOR: usize = 4;

/// `true` if the guard should escalate to a hard `deny` instead of the default non-blocking `ask`.
/// Strictly opt-in: only `COMPASS_GUARD_BLOCK` set to `1`/`true` enables it.
fn guard_block_enabled() -> bool {
    std::env::var("COMPASS_GUARD_BLOCK")
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// A clear, specific reason naming the file and why it's risky to edit — what the user sees in the
/// confirmation prompt.
fn guard_reason(rel: &str, a: &GuardAssessment) -> String {
    if a.is_hub && a.communities_bridged >= 2 {
        format!(
            "compass: {rel} is a hub — {} file(s) import it and it bridges {} parts of the codebase. \
             Confirm this edit before continuing (Compass guard is a convenience, not a guarantee).",
            a.dependents, a.communities_bridged
        )
    } else {
        // Pluralize the bare "imported by N" wording (it can read oddly for a degree-flagged hub
        // whose importers are exactly at the threshold), but keep it strictly about in-degree so it
        // never claims a dependency it can't substantiate.
        let files = if a.dependents == 1 { "file" } else { "files" };
        format!(
            "compass: {rel} is heavily depended on — {} {files} import it. \
             Confirm this edit before continuing (Compass guard is a convenience, not a guarantee).",
            a.dependents
        )
    }
}

/// Path of a session's guard "already-warned files" list under `.compass/sessions/`.
fn guard_warned_path(repo: &Path, session_id: &str) -> PathBuf {
    sessions_dir(repo).join(format!("{}.guard.json", safe_session_id(session_id)))
}

/// Files already warned about this session (so the guard doesn't nag twice), or empty if
/// none/unreadable. Best-effort — any error degrades to "warn again", never to a block.
fn load_guard_warned(repo: &Path, session_id: &str) -> Vec<String> {
    std::fs::read_to_string(guard_warned_path(repo, session_id))
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
        .unwrap_or_default()
}

/// Persist the session's warned-files list (best-effort; IO/serialize errors are ignored).
fn save_guard_warned(repo: &Path, session_id: &str, warned: &[String]) {
    let path = guard_warned_path(repo, session_id);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string(warned) {
        let _ = std::fs::write(path, json);
    }
}

fn print_help() {
    println!("compass — map a codebase into a queryable graph\n");
    println!("USAGE:");
    println!("  compass init [PATH]        Set up a repo: build the map + enable MCP (start here)");
    println!(
        "  compass install [PATH]     Wire Compass into AI hosts (--claude, --cursor, --codex, --all)"
    );
    println!(
        "                             (--guard adds an opt-in PreToolUse hub-edit confirmation hook)"
    );
    println!("  compass overview [PATH]    Summarize the repo map (default: current dir)");
    println!("  compass deps [PATH] <FILE> Show what a file imports and what imports it");
    println!("  compass broken [PATH]      List imports that point at missing files");
    println!("  compass watch [PATH]       Re-map the repo automatically as files change");
    println!("  compass map [PATH]         Open an interactive, live visual map in the browser");
    println!("                             (--port N, --no-open, --snapshot for a static .html)");
    println!(
        "  compass context [PATH]     Print a relevant map slice to pre-inject into an AI prompt"
    );
    println!("                             (--query \"task\" | --file PATH... | --hook; --max N, --fresh)");
    println!("  compass languages          List supported languages");
    println!("  compass serve [PATH]       Run the MCP server over stdio (for AI hosts)");
    println!(
        "  compass guard [PATH]       PreToolUse hook: confirm edits to high-centrality files"
    );
    println!(
        "                             (opt-in via `install --guard`; fails open, asks by default)"
    );
    println!("  compass help               Show this help");
}
