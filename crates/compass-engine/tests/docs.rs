//! Doc summaries end to end: real grammars, the shared comment reader in `docs`, and the graph.

use std::path::Path;

use compass_core::Graph;
use compass_extract::Registry;

fn index(files: &[(&str, &str)], name: &str) -> Graph {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (path, src) in files {
        std::fs::write(dir.join(path), src).unwrap();
    }
    let mut registry = Registry::new();
    registry.register(Box::new(compass_lang_rust::RustExtractor));
    registry.register(Box::new(compass_lang_typescript::TypeScriptExtractor));
    registry.register(Box::new(compass_lang_python::PythonExtractor));
    registry.register(Box::new(compass_lang_go::GoExtractor));
    let graph = compass_engine::index(&dir, &registry).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    graph
}

fn doc(graph: &Graph, symbol: &str) -> Option<String> {
    graph
        .symbols()
        .iter()
        .find(|s| s.name == symbol)
        .unwrap_or_else(|| panic!("{symbol} not mapped"))
        .doc
        .clone()
}

fn summary(graph: &Graph, path: &str) -> Option<String> {
    graph
        .files()
        .iter()
        .find(|f| f.path == Path::new(path))
        .unwrap_or_else(|| panic!("{path} not mapped"))
        .summary
        .clone()
}

#[test]
fn rust_docs_skip_attributes_and_inner_docs_describe_the_file() {
    let g = index(
        &[(
            "gate.rs",
            "//! Pre-trade risk checks. Every order passes here.\n\
             \n\
             /// Limits for one risk profile.\n\
             #[derive(Debug, Clone)]\n\
             pub struct RiskConfig {\n\
             \x20   /// Largest peak-to-trough loss before new buys stop.\n\
             \x20   pub max_drawdown: u8,\n\
             \x20   pub other: u8, // trailing, not a doc\n\
             }\n\
             \n\
             // A plain comment separated by a blank line.\n\
             \n\
             fn undocumented() {}\n",
        )],
        "docs-rust",
    );
    assert_eq!(
        summary(&g, "gate.rs").as_deref(),
        Some("Pre-trade risk checks.")
    );
    assert_eq!(
        doc(&g, "RiskConfig").as_deref(),
        Some("Limits for one risk profile.")
    );
    assert_eq!(
        doc(&g, "max_drawdown").as_deref(),
        Some("Largest peak-to-trough loss before new buys stop.")
    );
    assert_eq!(doc(&g, "other"), None);
    assert_eq!(doc(&g, "undocumented"), None);
}

#[test]
fn a_typescript_doc_block_above_an_export_documents_the_component() {
    let g = index(
        &[(
            "LiveEnablePanel.tsx",
            "import { useState } from 'react';\n\
             \n\
             /**\n\
             \x20* The live-enable gate panel: the deliberate real-money switch.\n\
             \x20*\n\
             \x20* @param props the panel props\n\
             \x20*/\n\
             export function LiveEnablePanel() { return null; }\n",
        )],
        "docs-ts",
    );
    assert_eq!(
        doc(&g, "LiveEnablePanel").as_deref(),
        Some("The live-enable gate panel: the deliberate real-money switch.")
    );
    // The block documents the component, and code (the import) comes first: no file summary.
    assert_eq!(summary(&g, "LiveEnablePanel.tsx"), None);
}

#[test]
fn python_docstrings_document_functions_classes_and_the_module() {
    let g = index(
        &[(
            "loader.py",
            "#!/usr/bin/env python\n\
             \"\"\"Load and validate the bot configuration.\"\"\"\n\
             \n\
             class Loader:\n\
             \x20   \"\"\"Reads TOML from disk.\n\
             \n\
             \x20   Longer notes.\n\
             \x20   \"\"\"\n\
             \n\
             \x20   def load(self):\n\
             \x20       '''Parse the file. Raises on errors.'''\n\
             \x20       return 1\n\
             \n\
             def plain():\n\
             \x20   \"x\"\n",
        )],
        "docs-py",
    );
    assert_eq!(
        summary(&g, "loader.py").as_deref(),
        Some("Load and validate the bot configuration.")
    );
    assert_eq!(doc(&g, "Loader").as_deref(), Some("Reads TOML from disk."));
    assert_eq!(doc(&g, "load").as_deref(), Some("Parse the file."));
    assert_eq!(
        doc(&g, "plain"),
        None,
        "a single-quoted string is not a docstring"
    );
}

#[test]
fn a_go_package_comment_describes_the_file_and_a_license_does_not() {
    let g = index(
        &[
            (
                "server.go",
                "// Copyright 2026 Example. Licensed under MIT.\n\
                 \n\
                 // Package server serves the dashboard API.\n\
                 package server\n\
                 \n\
                 // Start listens on addr until the context ends.\n\
                 func Start(addr string) {}\n",
            ),
            (
                "util.go",
                "// Copyright 2026 Example. Licensed under MIT.\n\
                 package util\n",
            ),
        ],
        "docs-go",
    );
    assert_eq!(
        summary(&g, "server.go").as_deref(),
        Some("Package server serves the dashboard API.")
    );
    assert_eq!(
        doc(&g, "Start").as_deref(),
        Some("Start listens on addr until the context ends.")
    );
    assert_eq!(summary(&g, "util.go"), None);
}

fn literals(graph: &Graph, path: &str) -> Vec<String> {
    graph
        .files()
        .iter()
        .find(|f| f.path == Path::new(path))
        .unwrap_or_else(|| panic!("{path} not mapped"))
        .literals
        .clone()
}

#[test]
fn short_literals_and_configuration_keys_are_kept_long_ones_and_comments_are_not() {
    let g = index(
        &[
            (
                "poll.ts",
                "// 'commented out'\n\
                 const defaultVoteOptions = ['Yes', 'No', 'Maybe'];\n\
                 export const qualityContent = { minEmojiCount: 5, 'quoted key': 1 };\n\
                 const long = 'this sentence is far too long to be a value anyone looks up by name at all';\n\
                 const multi = `a\nb`;\n",
            ),
            (
                "conf.go",
                "package conf\n\
                 var Defaults = Config{RetryInterval: 5, Name: \"relay\"}\n",
            ),
        ],
        "literals",
    );
    assert_eq!(
        literals(&g, "poll.ts"),
        ["Yes", "No", "Maybe", "minEmojiCount", "quoted key"]
    );
    assert_eq!(literals(&g, "conf.go"), ["RetryInterval", "Name", "relay"]);
}
