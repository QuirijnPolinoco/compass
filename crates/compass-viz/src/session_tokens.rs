//! Read-only aggregation of the local token-savings log (`compass context --hook`).
//!
//! The pre-injection hook records, per session, how much context it injected and how much it
//! kept from re-injecting via session de-dup, into `<repo>/.compass/sessions/<id>.tokens.json`
//! (written by `compass-cli`). This module folds every such file into one summary the `/tokens`
//! dashboard renders.
//!
//! Token counts are ESTIMATES (characters / 4), never exact — the `estimated` flag and the UI
//! say so. The honest, measurable number is the de-dup saving: tokens not re-injected for files
//! already shown this session. Everything here is local + read-only; nothing leaves the machine.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One injection event as written by the CLI hook. We read only the estimated token counts; the
/// writer (`compass-cli`'s `TokenEvent`) also records a timestamp and file counts the summary
/// ignores. `#[serde(default)]` keeps a partial or older record from poisoning the whole fold.
#[derive(Deserialize)]
struct RawTokenEvent {
    #[serde(default)]
    est_tokens_injected: u64,
    #[serde(default)]
    est_tokens_saved: u64,
}

/// Per-session rollup of estimated injected/saved tokens.
#[derive(Serialize)]
pub struct SessionTokens {
    pub id: String,
    pub injected_tokens: u64,
    pub saved_tokens: u64,
    pub injections: u64,
}

/// Whole-repo token-savings summary served at `/api/session-tokens`. Every token count here is
/// an estimate, surfaced by the always-true `estimated` flag.
#[derive(Serialize)]
pub struct SessionTokenSummary {
    /// Always `true`: token counts are estimates (chars / 4), not exact tokenizer counts.
    pub estimated: bool,
    pub total_injected_tokens: u64,
    pub total_saved_tokens: u64,
    pub total_injections: u64,
    pub sessions: Vec<SessionTokens>,
}

/// Fold every `<repo>/.compass/sessions/*.tokens.json` into one summary. Pure + read-only:
/// missing/unreadable/malformed files are skipped, and an absent directory yields all zeros.
pub fn aggregate_session_tokens(repo_root: &Path) -> SessionTokenSummary {
    let dir = repo_root.join(".compass").join("sessions");

    let mut sessions: Vec<SessionTokens> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(id) = path
                .file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_suffix(".tokens.json"))
            else {
                continue; // not a token log (e.g. the `<id>.json` seen-list)
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(events) = serde_json::from_str::<Vec<RawTokenEvent>>(&text) else {
                continue;
            };
            let injected_tokens: u64 = events.iter().map(|e| e.est_tokens_injected).sum();
            let saved_tokens: u64 = events.iter().map(|e| e.est_tokens_saved).sum();
            sessions.push(SessionTokens {
                id: id.to_string(),
                injected_tokens,
                saved_tokens,
                injections: events.len() as u64,
            });
        }
    }

    // Most-saved first (then id) for a stable, useful order in the dashboard table.
    sessions.sort_by(|a, b| {
        b.saved_tokens
            .cmp(&a.saved_tokens)
            .then_with(|| a.id.cmp(&b.id))
    });

    let total_injected_tokens: u64 = sessions.iter().map(|s| s.injected_tokens).sum();
    let total_saved_tokens: u64 = sessions.iter().map(|s| s.saved_tokens).sum();
    let total_injections: u64 = sessions.iter().map(|s| s.injections).sum();

    SessionTokenSummary {
        estimated: true,
        total_injected_tokens,
        total_saved_tokens,
        total_injections,
        sessions,
    }
}

/// Serialize [`aggregate_session_tokens`] for the `/api/session-tokens` route.
pub(crate) fn summary_json(repo_root: &Path) -> String {
    serde_json::to_string(&aggregate_session_tokens(repo_root)).unwrap_or_else(|_| {
        "{\"estimated\":true,\"total_injected_tokens\":0,\"total_saved_tokens\":0,\
         \"total_injections\":0,\"sessions\":[]}"
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregates_injected_saved_and_injection_counts() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let dir = tmp.path().join(".compass").join("sessions");
        std::fs::create_dir_all(&dir).unwrap();

        // Session "a": two events — 100 injected/0 saved, then 20 injected/80 saved.
        std::fs::write(
            dir.join("a.tokens.json"),
            r#"[
              {"at":1,"files_injected":3,"files_deduped":0,"est_tokens_injected":100,"est_tokens_saved":0},
              {"at":2,"files_injected":1,"files_deduped":2,"est_tokens_injected":20,"est_tokens_saved":80}
            ]"#,
        )
        .unwrap();
        // Session "b": one event — 50 injected / 40 saved.
        std::fs::write(
            dir.join("b.tokens.json"),
            r#"[{"at":3,"files_injected":2,"files_deduped":1,"est_tokens_injected":50,"est_tokens_saved":40}]"#,
        )
        .unwrap();
        // The seen-list file (same dir, different suffix) must be ignored.
        std::fs::write(dir.join("a.json"), r#"["src/main.rs"]"#).unwrap();

        let summary = aggregate_session_tokens(tmp.path());
        assert!(summary.estimated);
        assert_eq!(summary.total_injected_tokens, 170);
        assert_eq!(summary.total_saved_tokens, 120);
        assert_eq!(summary.total_injections, 3);
        assert_eq!(summary.sessions.len(), 2);

        let a = summary
            .sessions
            .iter()
            .find(|s| s.id == "a")
            .expect("session a");
        assert_eq!(a.injected_tokens, 120);
        assert_eq!(a.saved_tokens, 80);
        assert_eq!(a.injections, 2);

        let b = summary
            .sessions
            .iter()
            .find(|s| s.id == "b")
            .expect("session b");
        assert_eq!(b.injected_tokens, 50);
        assert_eq!(b.saved_tokens, 40);
        assert_eq!(b.injections, 1);

        // Most-saved session (a: 80) sorts before b (40).
        assert_eq!(summary.sessions[0].id, "a");
    }

    #[test]
    fn absent_dir_yields_zeros() {
        let tmp = tempfile::tempdir().expect("temp dir");
        // No `.compass/` at all under the repo root.
        let summary = aggregate_session_tokens(tmp.path());
        assert!(summary.estimated);
        assert_eq!(summary.total_injected_tokens, 0);
        assert_eq!(summary.total_saved_tokens, 0);
        assert_eq!(summary.total_injections, 0);
        assert!(summary.sessions.is_empty());
    }
}
