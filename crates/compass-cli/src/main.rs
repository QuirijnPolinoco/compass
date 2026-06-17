//! `compass` — CLI entrypoint and composition root.
//!
//! Builds the language registry via the explicit [`registry::register_all`] (ADR-0003),
//! runs the engine, and renders results (or serves them over MCP).

mod registry;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use compass_core::{Graph, MapQuery};

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
    println!("  compass languages          List supported languages");
    println!("  compass serve [PATH]       Run the MCP server over stdio (for AI hosts)");
    println!("  compass help               Show this help");
}
