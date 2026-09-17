//! `compass audit` — graph-driven health report: provable problems (cycles, broken imports)
//! vs. smells (hubs, isolated files).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use compass_core::{BrokenImport, HubFile, ImportCycle, MapQuery};

use crate::build_graph;

/// How many entries of each audit list the human-readable report prints before eliding the rest
/// with "… and N more". Keeps a large monorepo (hundreds of isolated/config files, many cycles)
/// from flooding the terminal. Override with `--limit N`; `--limit 0` shows everything. The `--json`
/// output is always complete (machine consumers want the full set).
const DEFAULT_AUDIT_LIMIT: usize = 20;

/// `compass audit [PATH] [--strict] [--json] [--limit N]` — report graph-driven code-health
/// findings, split honestly into provable defects and informational smells.
///
/// **Problems** are provable from the import graph: circular import dependencies (strongly-connected
/// file sets, each with a concrete cycle path) and broken imports (point at no real file). A cycle
/// that rests on a convention-based (heuristic) import edge is flagged for verification rather than
/// asserted. **Smells** are review hints, NOT asserted bugs — hub / "god" files and fully-isolated
/// files are frequently legitimate (shared utilities, entrypoints, config), so each is prefaced with
/// that caveat. The audit deliberately makes no unsound claims (no dead-code/unreferenced guessing).
///
/// Exit code is **0 by default** even when there are problems (the report is informational).
/// `--strict` exits non-zero when there are real problems — for CI — while smells never affect the
/// exit code. `--json` emits the full report as machine-readable JSON; `--limit` caps each list.
pub(crate) fn run_audit(args: &[String]) -> ExitCode {
    let mut path = PathBuf::from(".");
    let mut strict = false;
    let mut json = false;
    let mut limit = DEFAULT_AUDIT_LIMIT;

    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--strict" => strict = true,
            "--json" => json = true,
            "--limit" => match iter.next().and_then(|n| n.parse::<usize>().ok()) {
                Some(n) => limit = n,
                None => {
                    eprintln!("compass: --limit needs a number (0 = unlimited), e.g. `--limit 20`");
                    return ExitCode::FAILURE;
                }
            },
            other if other.starts_with("--limit=") => {
                match other["--limit=".len()..].parse::<usize>() {
                    Ok(n) => limit = n,
                    Err(_) => {
                        eprintln!(
                            "compass: --limit needs a number (0 = unlimited), e.g. `--limit=20`"
                        );
                        return ExitCode::FAILURE;
                    }
                }
            }
            other if other.starts_with('-') => {
                eprintln!("compass: unknown option `{other}` for `audit`");
                return ExitCode::FAILURE;
            }
            other => path = PathBuf::from(other),
        }
    }

    let Some(graph) = build_graph(&path) else {
        return ExitCode::FAILURE;
    };

    let cycles = graph.import_cycles();
    let broken = graph.broken_imports();
    let hubs = graph.hubs();
    let isolated = graph.isolated_files();
    let problem_count = cycles.len() + broken.len();

    if json {
        // Always-complete, machine-readable report (CI / tooling consume this beyond the exit code).
        let report = serde_json::json!({
            "path": path.display().to_string(),
            "summary": {
                "clean": problem_count == 0,
                "problem_count": problem_count,
                "circular_import_count": cycles.len(),
                "broken_import_count": broken.len(),
                "hub_count": hubs.len(),
                "isolated_file_count": isolated.len(),
            },
            "problems": {
                "circular_imports": cycles,
                "broken_imports": broken,
            },
            "smells": {
                "hubs": hubs,
                "isolated_files": isolated,
            },
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&report).unwrap_or_else(|e| format!(
                "{{\"error\":\"failed to serialize audit report: {e}\"}}"
            ))
        );
    } else {
        render_audit_text(&path, &cycles, &broken, &hubs, &isolated, limit);
    }

    // Only --strict fails CI, and only on real problems — smells never affect the exit code.
    if strict && problem_count > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// "1 file" / "3 files" — pluralize a count with a regular `-s`, so the audit's headline counts
/// read "1 problem" rather than "1 problem(s)".
fn plural(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

/// Print a "… and N more" elision line at `indent` when `hidden > 0` (a list was capped).
fn elide(hidden: usize, indent: &str) {
    if hidden > 0 {
        println!("{indent}… and {hidden} more (pass --limit 0 to show all)");
    }
}

/// Render one circular-import cluster as a concrete, actionable cycle path (a → b → … → a),
/// capped so a giant strongly-connected component can't flood a single line, and annotated when the
/// cycle rests on a convention-based (heuristic) import edge.
fn render_cycle(c: &ImportCycle) {
    if c.size == 1 {
        // A 1-file cycle is a file importing itself (defensive: no shipped extractor emits this).
        let file = c.path.first().or(c.files.first()).map(String::as_str);
        println!("    - {} imports itself", file.unwrap_or("?"));
        return;
    }

    // Show the concrete cycle, capping the rendered chain for very large SCCs.
    const MAX_PATH_NODES: usize = 12;
    let truncated = c.path.len() > MAX_PATH_NODES;
    let shown = if truncated {
        &c.path[..MAX_PATH_NODES]
    } else {
        &c.path[..]
    };
    let mut chain = shown.join(" → ");
    if truncated {
        chain.push_str(" → … (cycle continues)");
    } else if let Some(first) = c.path.first() {
        chain.push_str(" → ");
        chain.push_str(first); // close the loop back to the start
    }
    println!("    - cycle of {}: {chain}", plural(c.size, "file"));
    if c.heuristic {
        println!("        (rests on a convention-based import edge — verify before acting)");
    }
}

/// Render the human-readable audit report. Each list is capped to `limit` entries (`0` = unlimited)
/// with a "… and N more" elision, and the explanatory smell caveats are printed only when the
/// corresponding list is non-empty (so a clean repo doesn't emit boilerplate).
fn render_audit_text(
    path: &Path,
    cycles: &[ImportCycle],
    broken: &[BrokenImport],
    hubs: &[HubFile],
    isolated: &[String],
    limit: usize,
) {
    let cap = |total: usize| if limit == 0 { total } else { total.min(limit) };
    let problem_count = cycles.len() + broken.len();

    println!("Compass audit — {}", path.display());
    println!();

    // ---- Problems: graph-provable defects (heuristic cycles flagged individually). ----------
    println!("Problems (provable from the import graph):");
    if cycles.is_empty() {
        println!("  Circular imports: none");
    } else {
        println!("  Circular imports ({}):", cycles.len());
        let shown = cap(cycles.len());
        for c in &cycles[..shown] {
            render_cycle(c);
        }
        elide(cycles.len() - shown, "    ");
    }
    if broken.is_empty() {
        println!("  Broken imports: none");
    } else {
        println!("  Broken imports ({}):", broken.len());
        let shown = cap(broken.len());
        for b in &broken[..shown] {
            println!("    - {} — {}", b.file, b.message);
        }
        elide(broken.len() - shown, "    ");
    }
    println!();

    // ---- Smells: review hints, explicitly NOT asserted defects. -----------------------------
    println!("Smells (review, not necessarily bugs):");
    if hubs.is_empty() {
        println!("  God-files / hubs: none");
    } else {
        println!(
            "  God-files / hubs ({}) — files that bridge many parts of the codebase.",
            hubs.len()
        );
        println!(
            "    Often legitimate (a shared util or a public API surface); review only one that"
        );
        println!("    keeps churning or accreting unrelated responsibilities.");
        let shown = cap(hubs.len());
        for h in &hubs[..shown] {
            println!(
                "    - {} — bridges {} communities, {}",
                h.file,
                h.communities_bridged,
                plural(h.degree, "import connection")
            );
        }
        elide(hubs.len() - shown, "    ");
    }
    if isolated.is_empty() {
        println!("  Isolated files: none");
    } else {
        println!(
            "  Isolated files ({}) — no resolved import edges in or out.",
            isolated.len()
        );
        println!(
            "    Often legitimate entrypoints, config, generated code, or standalone scripts;"
        );
        println!("    flagged only so an unexpectedly disconnected file doesn't hide.");
        let shown = cap(isolated.len());
        for f in &isolated[..shown] {
            println!("    - {f}");
        }
        elide(isolated.len() - shown, "    ");
    }
    println!();

    // ---- Summary (always complete counts, even when the lists above were capped). -----------
    let smells = format!(
        "{}, {} noted as smells",
        plural(hubs.len(), "hub"),
        plural(isolated.len(), "isolated file")
    );
    if problem_count == 0 {
        println!("Summary: clean — no circular or broken imports. ({smells}.)");
    } else {
        println!(
            "Summary: {} — {}, {}. ({smells}.)",
            plural(problem_count, "problem"),
            plural(cycles.len(), "circular-import cluster"),
            plural(broken.len(), "broken import"),
        );
    }
}
