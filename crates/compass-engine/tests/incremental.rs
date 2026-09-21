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

#[test]
fn a_cached_extraction_for_an_unregistered_language_is_not_reused() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("incr-language-set");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();

    let (_, cache) = compass_engine::index_incremental(&dir, &registry(), None).unwrap();
    assert!(cache.contains_key("a.rs"));

    // A build without the Rust extractor sees the same cache. a.rs is unchanged, but this
    // registry could never have produced that extraction, so the file must not be mapped.
    let (graph, _) =
        compass_engine::index_incremental(&dir, &Registry::new(), Some(&cache)).unwrap();
    assert!(graph.files().is_empty(), "files: {:?}", graph.files());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn caches_written_by_another_release_are_discarded() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("incr-producer");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();

    let (graph, extractions) = compass_engine::index_incremental(&dir, &registry(), None).unwrap();
    compass_engine::cache::save(&dir, &graph).unwrap();
    compass_engine::cache::save_extractions(&dir, &extractions).unwrap();
    assert!(compass_engine::cache::load(&dir).is_some());
    assert!(compass_engine::cache::load_extractions(&dir).is_some());

    // Re-stamp both files as another release's output (same format version). An upgrade must
    // not keep serving the old release's extraction of files that haven't changed since.
    for file in ["graph.json", "extractions.json"] {
        let path = dir.join(".compass").join(file);
        let mut json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        json["producer"] = serde_json::json!("0.0.0-other");
        std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    }
    assert!(compass_engine::cache::load(&dir).is_none());
    assert!(compass_engine::cache::load_extractions(&dir).is_none());

    // A cache from before the stamp existed (no `producer` key) is discarded the same way.
    let path = dir.join(".compass").join("extractions.json");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    json.as_object_mut().unwrap().remove("producer");
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(compass_engine::cache::load_extractions(&dir).is_none());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_graph_cached_before_categories_existed_loads_as_code() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("incr-category-default");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.rs"), "fn a() {}\n").unwrap();

    let (graph, _) = compass_engine::index_incremental(&dir, &registry(), None).unwrap();
    compass_engine::cache::save(&dir, &graph).unwrap();

    // Strip the field an older build never wrote; every file it mapped was source code.
    let path = dir.join(".compass").join("graph.json");
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    json["graph"]["files"][0]
        .as_object_mut()
        .expect("file object")
        .remove("category")
        .expect("category is serialized");
    std::fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();

    let restored = compass_engine::cache::load(&dir).expect("still loads");
    assert_eq!(
        restored.files()[0].category,
        compass_core::FileCategory::code()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_extensionless_script_is_mapped_by_its_shebang_and_reused_from_cache() {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("incr-shebang");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    std::fs::write(
        dir.join("bin").join("cli"),
        "#!/usr/bin/env node\nfunction main() {}\nmain();\n",
    )
    .unwrap();
    // Extensionless, but no `#!` line: not a script anyone can identify.
    std::fs::write(dir.join("LICENSE"), "MIT\n").unwrap();

    let mut registry = Registry::new();
    registry.register(Box::new(compass_lang_typescript::TypeScriptExtractor));

    let (g1, cache) = compass_engine::index_incremental(&dir, &registry, None).unwrap();
    let paths: Vec<String> = g1
        .files()
        .iter()
        .map(|f| f.path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert_eq!(paths, ["bin/cli"]);
    assert_eq!(symbol_names(&g1), ["main"]);

    // Unchanged on the next run: served from the cache (it has no extension to re-detect by).
    let (g2, _) = compass_engine::index_incremental(&dir, &registry, Some(&cache)).unwrap();
    assert_eq!(symbol_names(&g2), ["main"]);

    let _ = std::fs::remove_dir_all(&dir);
}
