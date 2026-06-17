//! Repo walking + path normalization. Respects `.gitignore` via the `ignore` crate
//! (FR-9/A3). Language detection itself is registry-driven (architecture §9) and happens
//! in [`crate::index`], not here — this module only yields candidate files.

use std::path::{Path, PathBuf};

/// A discovered file: its absolute path (to read from disk) and its repo-relative,
/// forward-slash-normalized path (the stable identity used in the graph).
pub struct Walked {
    pub abs: PathBuf,
    pub rel: PathBuf,
}

/// Walk `repo_root`, honoring `.gitignore`, returning every regular file.
pub fn walk(repo_root: &Path) -> Vec<Walked> {
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(repo_root).build().flatten() {
        if entry.file_type().is_some_and(|t| t.is_file()) {
            let abs = entry.path().to_path_buf();
            if let Ok(rel) = abs.strip_prefix(repo_root) {
                out.push(Walked {
                    rel: normalize(rel),
                    abs,
                });
            }
        }
    }
    out
}

/// Normalize a relative path to use `/` separators, so the graph's identities and
/// output are identical across Windows/macOS/Linux.
pub fn normalize(rel: &Path) -> PathBuf {
    let joined = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    PathBuf::from(joined)
}

/// The parent directory of a normalized repo-relative path (empty path for repo root).
pub fn parent_dir(rel: &Path) -> PathBuf {
    match rel.to_string_lossy().rsplit_once('/') {
        Some((dir, _)) => PathBuf::from(dir),
        None => PathBuf::new(),
    }
}
