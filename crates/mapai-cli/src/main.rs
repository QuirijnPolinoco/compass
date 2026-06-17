//! `mapai` — CLI entrypoint and composition root.
//!
//! Builds the language registry via the explicit [`registry::register_all`] (ADR-0003),
//! runs the engine, and renders results. The MCP `serve` command is wired in the next slice.

mod registry;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mapai_core::MapQuery;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "overview".to_string());
    let path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    match command.as_str() {
        "overview" => run_overview(&path),
        "languages" => run_languages(),
        "help" | "-h" | "--help" => {
            print_help();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("mapai: unknown command `{other}`\n");
            print_help();
            ExitCode::FAILURE
        }
    }
}

fn run_overview(path: &Path) -> ExitCode {
    let registry = registry::register_all();
    let graph = match mapai_engine::index(path, &registry) {
        Ok(graph) => graph,
        Err(e) => {
            eprintln!("mapai: failed to index {}: {e:#}", path.display());
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = mapai_engine::cache::save(path, &graph) {
        eprintln!("mapai: warning: could not write cache: {e:#}");
    }

    let overview = graph.overview();
    println!("MapAI overview — {}", path.display());
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

fn print_help() {
    println!("mapai — map a codebase into a queryable graph\n");
    println!("USAGE:");
    println!("  mapai overview [PATH]    Summarize the repo map (default: current dir)");
    println!("  mapai languages          List supported languages");
    println!("  mapai help               Show this help");
}
