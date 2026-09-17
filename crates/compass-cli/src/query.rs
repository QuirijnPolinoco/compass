//! The read-only query commands: `overview`, `deps`, `broken`, `languages`, and `serve` (the
//! same queries over MCP).

use std::path::Path;
use std::process::ExitCode;

use compass_core::MapQuery;

use crate::{build_graph, registry};

pub(crate) fn run_overview(path: &Path) -> ExitCode {
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

pub(crate) fn run_deps(path: &Path, file: Option<&str>) -> ExitCode {
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

pub(crate) fn run_broken(path: &Path) -> ExitCode {
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

pub(crate) fn run_languages() -> ExitCode {
    let registry = registry::register_all();
    let ids = registry.language_ids();
    println!("Supported languages ({}):", ids.len());
    for id in ids {
        println!("  - {id}");
    }
    ExitCode::SUCCESS
}

pub(crate) fn run_serve(path: &Path) -> ExitCode {
    let Some(graph) = build_graph(path) else {
        return ExitCode::FAILURE;
    };
    let query: std::sync::Arc<dyn MapQuery + Send + Sync> = std::sync::Arc::new(graph);
    let supported_languages = registry::register_all()
        .language_ids()
        .iter()
        .map(ToString::to_string)
        .collect();
    if let Err(e) = compass_mcp::serve_stdio(query, supported_languages) {
        eprintln!("compass: MCP server error: {e:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
