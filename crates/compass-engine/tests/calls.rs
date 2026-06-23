//! End-to-end check of the engine's call resolution (`index::resolve_calls`).
//!
//! The per-language extraction of raw caller→callee names is unit-tested inside each
//! `compass-lang-*` crate; this drives the whole pipeline (walk → extract → assemble →
//! resolve) on a throwaway Rust repo and asserts that raw names become `SymbolId` call edges
//! under the conservative rule: same-file match first, else a *unique* global match, and
//! ambiguous names are skipped (never a guessed edge).

use std::collections::HashMap;
use std::path::PathBuf;

use compass_core::{EdgeConfidence, Graph};
use compass_extract::Registry;

/// Build a registry with only the Rust extractor and index `dir`.
fn index_rust(dir: &std::path::Path) -> Graph {
    let mut registry = Registry::new();
    registry.register(Box::new(compass_lang_rust::RustExtractor));
    compass_engine::index(dir, &registry).expect("index")
}

/// Translate the resolved `(caller, callee)` symbol-id edges into readable
/// `(caller_name, "file::callee_name")` pairs so assertions can pin the *exact* target file
/// (names alone are ambiguous — that's the whole point of the test).
fn readable_calls(graph: &Graph) -> Vec<(String, String)> {
    let mut by_id = HashMap::new();
    for s in graph.symbols() {
        let file = graph
            .file_path(s.file)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        by_id.insert(s.id, (file, s.name.clone()));
    }
    let mut edges: Vec<(String, String)> = graph
        .calls()
        .iter()
        .map(|&(from, to, _)| {
            let (_, caller) = &by_id[&from];
            let (file, callee) = &by_id[&to];
            (caller.clone(), format!("{file}::{callee}"))
        })
        .collect();
    edges.sort();
    edges
}

/// Like [`readable_calls`], but also carries each edge's [`EdgeConfidence`] so a test can
/// assert that a same-file call is `Resolved` and a unique-global call is `Heuristic`.
fn readable_calls_with_confidence(graph: &Graph) -> Vec<(String, String, EdgeConfidence)> {
    let mut by_id = HashMap::new();
    for s in graph.symbols() {
        let file = graph
            .file_path(s.file)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        by_id.insert(s.id, (file, s.name.clone()));
    }
    let mut edges: Vec<(String, String, EdgeConfidence)> = graph
        .calls()
        .iter()
        .map(|&(from, to, confidence)| {
            let (_, caller) = &by_id[&from];
            let (file, callee) = &by_id[&to];
            (caller.clone(), format!("{file}::{callee}"), confidence)
        })
        .collect();
    // EdgeConfidence isn't Ord, so sort by the readable (caller, target) pair only.
    edges.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    edges
}

#[test]
fn resolves_same_file_unique_global_and_skips_ambiguous() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("calls-resolution");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // a.rs: `helper` and `shared` are defined here; `alpha` calls both (same-file).
    std::fs::write(
        dir.join("a.rs"),
        "fn helper() {}\nfn shared() {}\nfn alpha() { helper(); shared(); }\n",
    )
    .unwrap();
    // b.rs: `shared` is ALSO defined here. `beta` calls `helper` (only in a.rs → unique global,
    // cross-file). `gamma` calls `shared` → same-file b.rs wins over a.rs's `shared`.
    std::fs::write(
        dir.join("b.rs"),
        "fn shared() {}\nfn beta() { helper(); }\nfn gamma() { shared(); }\n",
    )
    .unwrap();
    // c.rs: `orphan` calls `shared`, which is defined in BOTH a.rs and b.rs and NOT here →
    // ambiguous (2 global matches, no local) → skipped, no edge.
    std::fs::write(dir.join("c.rs"), "fn orphan() { shared(); }\n").unwrap();

    let graph = index_rust(&dir);
    let calls = readable_calls(&graph);

    assert_eq!(
        calls,
        [
            // same-file: a.rs's `alpha` → a.rs's `helper`/`shared`
            ("alpha".to_string(), "a.rs::helper".to_string()),
            ("alpha".to_string(), "a.rs::shared".to_string()),
            // unique global: b.rs's `beta` → the only `helper` (in a.rs)
            ("beta".to_string(), "a.rs::helper".to_string()),
            // same-file beats the cross-file duplicate: b.rs's `gamma` → b.rs's `shared`
            ("gamma".to_string(), "b.rs::shared".to_string()),
            // NOTE: c.rs's `orphan` → `shared` is absent — ambiguous, correctly skipped.
        ],
        "resolved calls (orphan→shared must be dropped as ambiguous):\n{calls:#?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn same_file_call_is_resolved_and_unique_global_call_is_heuristic() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("calls-confidence");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // a.rs: `helper` is defined here; `alpha` calls it (same-file → Resolved).
    std::fs::write(
        dir.join("a.rs"),
        "fn helper() {}\nfn alpha() { helper(); }\n",
    )
    .unwrap();
    // b.rs: `beta` calls `helper`, which is defined only in a.rs → unique global → Heuristic.
    std::fs::write(dir.join("b.rs"), "fn beta() { helper(); }\n").unwrap();

    let graph = index_rust(&dir);
    let calls = readable_calls_with_confidence(&graph);

    assert_eq!(
        calls,
        [
            // same-file: a.rs's `alpha` → a.rs's `helper` is provable → Resolved.
            (
                "alpha".to_string(),
                "a.rs::helper".to_string(),
                EdgeConfidence::Resolved,
            ),
            // unique global: b.rs's `beta` → the only `helper` (in a.rs) is a guess → Heuristic.
            (
                "beta".to_string(),
                "a.rs::helper".to_string(),
                EdgeConfidence::Heuristic,
            ),
        ],
        "call-edge confidence (same-file Resolved, unique-global Heuristic):\n{calls:#?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
