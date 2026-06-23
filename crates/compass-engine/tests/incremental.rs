//! End-to-end checks for incremental indexing (`index::index_incremental`): a file whose
//! `(mtime, size)` are unchanged must be reused from the cache (never re-read), while changed,
//! added, and deleted files are always reflected correctly.

use std::path::Path;

use compass_core::{Graph, Span, SymbolKind};
use compass_extract::{ExtractedSymbol, Registry};

fn registry() -> Registry {
    let mut r = Registry::new();
    r.register(Box::new(compass_lang_rust::RustExtractor));
    r
}

/// Sorted symbol names in the graph — the observable we assert on.
fn symbol_names(graph: &Graph) -> Vec<String> {
    let mut names: Vec<String> = graph.symbols().iter().map(|s| s.name.clone()).collect();
    names.sort();
    names
}

#[test]
fn unchanged_files_are_reused_from_cache_without_rereading() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("incr-reuse");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("b.rs"), "fn original() {}\n").unwrap();

    // First index builds the extraction cache.
    let (g1, mut cache) = compass_engine::index_incremental(&dir, &registry(), None).unwrap();
    assert_eq!(symbol_names(&g1), ["original"]);

    // Poison the cached entry for b.rs WITHOUT touching the file (so its fingerprint still
    // matches disk). If the second index re-read b.rs it would see `original`; if it trusts the
    // cache (the whole point) it returns the poisoned symbol. b.rs is unchanged → must reuse.
    let entry = cache.get_mut("b.rs").expect("b.rs cached");
    entry.symbols = vec![ExtractedSymbol {
        name: "poisoned".to_string(),
        kind: SymbolKind::Function,
        span: Span {
            start_byte: 0,
            end_byte: 0,
            start_row: 0,
            start_col: 0,
        },
    }];

    let (g2, _) = compass_engine::index_incremental(&dir, &registry(), Some(&cache)).unwrap();
    assert_eq!(
        symbol_names(&g2),
        ["poisoned"],
        "an unchanged file must be reused from the cache, not re-parsed"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn changed_added_and_deleted_files_are_reflected() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("incr-change");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.rs"), "fn one() {}\n").unwrap();
    std::fs::write(dir.join("keep.rs"), "fn kept() {}\n").unwrap();

    let (_, cache) = compass_engine::index_incremental(&dir, &registry(), None).unwrap();

    // Change a.rs (different content → different size → fingerprint differs → re-parsed),
    // delete keep.rs, and add c.rs. Poison keep.rs's cache entry to prove a *deleted* file is
    // dropped (not resurrected from cache).
    std::fs::write(dir.join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
    std::fs::remove_file(dir.join("keep.rs")).unwrap();
    std::fs::write(dir.join("c.rs"), "fn three() {}\n").unwrap();
    let mut cache = cache;
    cache.get_mut("keep.rs").unwrap().symbols[0].name = "should_not_appear".to_string();

    let (g2, new_cache) =
        compass_engine::index_incremental(&dir, &registry(), Some(&cache)).unwrap();
    assert_eq!(
        symbol_names(&g2),
        ["one", "three", "two"],
        "changed file re-parsed (one+two), added file picked up (three), deleted file gone"
    );
    // The refreshed cache tracks exactly the current file set.
    let keys: std::collections::BTreeSet<&str> = new_cache.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["a.rs", "c.rs"].into_iter().collect(),
        "the new cache drops the deleted file and adds the new one"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_prev_cache_is_a_full_index() {
    // index_incremental(None) must behave exactly like a full index.
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("incr-none");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("x.rs"), "fn alpha() {}\nfn beta() {}\n").unwrap();

    let plain = compass_engine::index(&dir, &registry()).unwrap();
    let (incremental, cache) = compass_engine::index_incremental(&dir, &registry(), None).unwrap();
    assert_eq!(symbol_names(&plain), symbol_names(&incremental));
    assert_eq!(cache.len(), 1, "one file cached");

    let _ = std::fs::remove_dir_all(&dir);
}
