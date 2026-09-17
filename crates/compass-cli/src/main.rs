//! `compass` — CLI entrypoint and composition root.
//!
//! Builds the language registry via the explicit [`registry::register_all`] (ADR-0003),
//! runs the engine, and renders results (or serves them over MCP).

mod audit;
mod context;
mod guard;
mod install;
mod map;
mod query;
mod registry;
mod session;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use compass_core::Graph;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("overview");
    let path = args
        .get(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    match command {
        "init" => install::run_init(&path),
        "install" => install::run_install(&args[1..]),
        "overview" => query::run_overview(&path),
        "languages" => query::run_languages(),
        "deps" => query::run_deps(&path, args.get(2).map(String::as_str)),
        "broken" => query::run_broken(&path),
        "audit" => audit::run_audit(&args[1..]),
        "watch" => map::run_watch(&path),
        "map" => map::run_map(&args[1..]),
        "context" => context::run_context(&args[1..]),
        "guard" => guard::run_guard(&path),
        "serve" => query::run_serve(&path),
        "help" | "-h" | "--help" => {
            print_help();
            ExitCode::SUCCESS
        }
        "version" | "-V" | "--version" => {
            println!("compass {}", env!("CARGO_PKG_VERSION"));
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
pub(crate) fn build_graph(path: &Path) -> Option<Graph> {
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

/// Absolute path as a clean forward-slash string (strips Windows `\\?\` verbatim prefix).
pub(crate) fn clean_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    s.strip_prefix(r"\\?\").unwrap_or(&s).replace('\\', "/")
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
    println!(
        "  compass audit [PATH]       Report code-health findings: cycles & broken imports (provable),"
    );
    println!(
        "                             plus hub/isolated-file smells. Exits 0 by default even with"
    );
    println!(
        "                             problems; --strict exits non-zero on problems (for CI). Also"
    );
    println!(
        "                             --json (full machine-readable report), --limit N (0 = all)"
    );
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
    println!("  compass version            Print the version (also --version, -V)");
}
