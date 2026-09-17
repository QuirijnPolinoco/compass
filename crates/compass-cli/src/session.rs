//! Per-session state files under `.compass/sessions/`, shared by `context` (what was already
//! injected) and `guard` (what was already warned about).

use std::path::{Path, PathBuf};

/// A repo's `.compass/sessions/` directory, where per-session state lives.
pub(crate) fn sessions_dir(repo: &Path) -> PathBuf {
    repo.join(".compass").join("sessions")
}

/// Filename-safe form of a host-generated session id (UUIDs); keep only safe chars defensively.
/// The seen-list (`<id>.json`) and the token log (`<id>.tokens.json`) share this same key.
pub(crate) fn safe_session_id(session_id: &str) -> String {
    session_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
