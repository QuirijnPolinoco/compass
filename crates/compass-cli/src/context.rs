//! `compass context` — the map slice pre-injected into an AI prompt (ADR-0006), its session
//! de-duplication, and the token-savings log.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use compass_core::{ContextPack, ContextRequest, MapQuery};
use serde::{Deserialize, Serialize};

use crate::build_graph;
use crate::session::{safe_session_id, sessions_dir};

/// `compass context` — print a token-bounded context pack for **pre-injection** into an AI
/// prompt (ADR-0006): a structural summary + the most relevant files. `--query` ranks by the
/// task text; `--file` seeds the blast-radius around files being worked on; otherwise the
/// most-connected files are returned.
pub(crate) fn run_context(args: &[String]) -> ExitCode {
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
        // Only worth the tokens when the blast radius reaches past the importers just listed.
        if f.affected_count > f.dependents.len() {
            let _ = write!(line, " — a change here affects {} files", f.affected_count);
        }
        let _ = writeln!(out, "{line}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use compass_core::ContextFile;

    fn pack_with(file: ContextFile) -> ContextPack {
        ContextPack {
            file_count: 1,
            languages: Vec::new(),
            most_connected: Vec::new(),
            selected_by: "seeds".to_string(),
            files: vec![file],
        }
    }

    fn file(dependents: &[&str], affected_count: usize) -> ContextFile {
        ContextFile {
            path: "src/util.rs".to_string(),
            language: Some("rust".to_string()),
            symbols: Vec::new(),
            depends_on: Vec::new(),
            dependents: dependents.iter().map(ToString::to_string).collect(),
            affected_count,
        }
    }

    #[test]
    fn blast_radius_is_rendered_only_when_it_reaches_past_the_listed_importers() {
        let deep = render_context_markdown(Path::new("."), &pack_with(file(&["a.rs"], 9)));
        assert!(
            deep.contains("imported by: a.rs — a change here affects 9 files"),
            "{deep}"
        );

        let shallow = render_context_markdown(Path::new("."), &pack_with(file(&["a.rs"], 1)));
        assert!(!shallow.contains("affects"), "{shallow}");
    }
}
