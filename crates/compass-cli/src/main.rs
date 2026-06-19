//! `compass` — CLI entrypoint and composition root.
//!
//! Builds the language registry via the explicit [`registry::register_all`] (ADR-0003),
//! runs the engine, and renders results (or serves them over MCP).

mod registry;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use compass_core::{ContextPack, ContextRequest, Graph, MapQuery};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("overview");
    let path = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    match command {
        "init" => run_init(&path),
        "overview" => run_overview(&path),
        "languages" => run_languages(),
        "deps" => run_deps(&path, args.get(2).map(String::as_str)),
        "broken" => run_broken(&path),
        "watch" => run_watch(&path),
        "map" => run_map(&args[1..]),
        "context" => run_context(&args[1..]),
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
fn build_graph(path: &Path) -> Option<Graph> {
    let registry = registry::register_all();
    match compass_engine::index(path, &registry) {
        Ok(graph) => Some(graph),
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
        let state = compass_viz::MapState::new(Arc::new(graph));
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

    if hook {
        // UserPromptSubmit payload: { "prompt": "...", "cwd": "...", ... }. Pull the prompt
        // (as the query) and the cwd (if no PATH was given). Anything missing → just skip.
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
    let pack = graph.context(&ContextRequest {
        query,
        seeds,
        max_files,
    });
    print!("{}", render_context_markdown(&path, &pack));
    ExitCode::SUCCESS
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

fn print_help() {
    println!("compass — map a codebase into a queryable graph\n");
    println!("USAGE:");
    println!("  compass init [PATH]        Set up a repo: build the map + enable MCP (start here)");
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
    println!("  compass help               Show this help");
}
