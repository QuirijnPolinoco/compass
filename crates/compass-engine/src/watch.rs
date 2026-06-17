//! Live freshness (FR-13/F1): watch a repo and react to debounced file changes.
//!
//! Build/VCS/cache directories are ignored so that writing our own `.compass/` cache can't
//! trigger a re-index loop. v1 re-indexes the whole repo on change; incremental
//! single-file reparse is a planned optimization.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use notify_debouncer_mini::notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};

/// Directory names whose changes we never react to (our cache, VCS, build output).
const IGNORED_DIRS: [&str; 3] = [".git", ".compass", "target"];

/// Watch `repo` recursively, calling `on_change` with the changed paths after each
/// debounce window. Blocks until the process is interrupted (Ctrl+C).
pub fn watch<F>(repo: &Path, debounce: Duration, mut on_change: F) -> anyhow::Result<()>
where
    F: FnMut(&[PathBuf]),
{
    let (tx, rx) = mpsc::channel();
    let mut debouncer = new_debouncer(debounce, move |res: DebounceEventResult| {
        let _ = tx.send(res);
    })?;
    debouncer.watcher().watch(repo, RecursiveMode::Recursive)?;

    for events in rx.into_iter().flatten() {
        let paths: Vec<PathBuf> = events
            .into_iter()
            .map(|e| e.path)
            .filter(|p| !is_ignored(p))
            .collect();
        if !paths.is_empty() {
            on_change(&paths);
        }
    }
    Ok(())
}

fn is_ignored(path: &Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .map(|s| IGNORED_DIRS.contains(&s))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_cache_vcs_and_build_paths() {
        assert!(is_ignored(Path::new("repo/.compass/graph.json")));
        assert!(is_ignored(Path::new("repo/.git/HEAD")));
        assert!(is_ignored(Path::new("target/debug/compass")));
        assert!(!is_ignored(Path::new("src/main.go")));
    }
}
