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
    if overview.not_analysed_count > 0 {
        println!(
            "  not analysed: {} (too large, minified or generated — mapped, contents skipped)",
            overview.not_analysed_count
        );
    }
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
    if !overview.supporting.is_empty() {
        println!("  supporting files:");
        for stat in &overview.supporting {
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

/// What this build maps, from the one registry everything else uses: the languages (code-like
/// extractors) and the supporting file types, each in registration order.
fn supported() -> (Vec<String>, Vec<String>) {
    let registry = registry::register_all();
    let (code, supporting): (Vec<_>, Vec<_>) = registry
        .extractors()
        .iter()
        .partition(|e| e.category().is_code_like());
    let ids = |extractors: Vec<&Box<dyn compass_extract::Extractor>>| {
        extractors
            .iter()
            .map(|e| e.language_id().to_string())
            .collect()
    };
    (ids(code), ids(supporting))
}

pub(crate) fn run_languages() -> ExitCode {
    let (languages, file_types) = supported();
    println!("Supported languages ({}):", languages.len());
    for id in &languages {
        println!("  - {id}");
    }
    // Supporting (non-code) file types are listed apart: they are mapped, but they are not
    // languages and never take part in dependency metrics (ADR-0007).
    if !file_types.is_empty() {
        println!("Supporting file types ({}):", file_types.len());
        for id in &file_types {
            println!("  - {id}");
        }
    }
    ExitCode::SUCCESS
}

pub(crate) fn run_serve(path: &Path) -> ExitCode {
    let Some(graph) = build_graph(path) else {
        return ExitCode::FAILURE;
    };
    let query: std::sync::Arc<dyn MapQuery + Send + Sync> = std::sync::Arc::new(graph);
    let (languages, file_types) = supported();
    if let Err(e) = compass_mcp::serve_stdio(query, languages, file_types) {
        eprintln!("compass: MCP server error: {e:#}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
