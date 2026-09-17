//! `compass guard` — the opt-in PreToolUse hook that asks before an edit to a high-centrality
//! file. Fails open: any error lets the edit through.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use compass_core::{EdgeKind, Graph, MapQuery, NodeKind};

use crate::clean_path;
use crate::session::{safe_session_id, sessions_dir};

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
pub(crate) fn run_guard(path: &Path) -> ExitCode {
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
    /// Every file a change can reach — direct *and* transitive dependents. Reported in the
    /// reason only; selection stays on `dependents` (see [`assess_centrality`]).
    affected: usize,
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

    // Same economy for the transitive walk: it only ever shows up in a warning.
    let affected = if high_centrality {
        graph.impact(rel).map_or(dependents, |i| i.total_count)
    } else {
        dependents
    };

    Some(GuardAssessment {
        high_centrality,
        is_hub,
        dependents,
        communities_bridged,
        affected,
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
    // The transitive reach, when it goes beyond the direct importers already named.
    let downstream = if a.affected > a.dependents {
        format!(" ({} files affected downstream)", a.affected)
    } else {
        String::new()
    };
    if a.is_hub && a.communities_bridged >= 2 {
        format!(
            "compass: {rel} is a hub — {} file(s) import it{downstream} and it bridges {} parts of \
             the codebase. Confirm this edit before continuing (Compass guard is a convenience, \
             not a guarantee).",
            a.dependents, a.communities_bridged
        )
    } else {
        // Pluralize the bare "imported by N" wording (it can read oddly for a degree-flagged hub
        // whose importers are exactly at the threshold), but keep it strictly about in-degree so it
        // never claims a dependency it can't substantiate.
        let files = if a.dependents == 1 { "file" } else { "files" };
        format!(
            "compass: {rel} is heavily depended on — {} {files} import it{downstream}. \
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

#[cfg(test)]
mod tests {
    use super::*;

    fn assessment(dependents: usize, affected: usize, bridged: usize) -> GuardAssessment {
        GuardAssessment {
            high_centrality: true,
            is_hub: bridged >= 2,
            dependents,
            communities_bridged: bridged,
            affected,
        }
    }

    #[test]
    fn reason_names_the_downstream_reach_only_when_it_exceeds_the_importers() {
        let deep = guard_reason("src/util.rs", &assessment(6, 41, 0));
        assert!(
            deep.contains("6 files import it (41 files affected downstream)."),
            "{deep}"
        );

        // Every affected file is a direct importer -> nothing extra to say.
        let shallow = guard_reason("src/util.rs", &assessment(6, 6, 0));
        assert!(shallow.contains("6 files import it."), "{shallow}");
        assert!(!shallow.contains("downstream"), "{shallow}");
    }

    #[test]
    fn hub_reason_keeps_the_bridging_clause_after_the_downstream_reach() {
        let reason = guard_reason("src/util.rs", &assessment(3, 20, 3));
        assert!(
            reason.contains(
                "3 file(s) import it (20 files affected downstream) and it bridges 3 parts"
            ),
            "{reason}"
        );
    }
}
