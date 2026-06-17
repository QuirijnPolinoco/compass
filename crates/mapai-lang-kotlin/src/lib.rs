//! `mapai-lang-kotlin` — the Kotlin language extractor.
//!
//! A self-contained unit behind the [`mapai_extract::Extractor`] trait (ADR-0002):
//! detection (.kt/.kts), the `tree-sitter-kotlin-ng` grammar, symbol extraction (classes,
//! interfaces, enums, objects, functions/methods), and package-aware import resolution.
//!
//! Kotlin doesn't require a file's name to match its classes, so import resolution is a
//! best-effort convention (package mirrors folder; `import a.b.C` -> `a/b/C.kt` under the
//! source root). Unresolved imports are treated as external, never flagged broken.

use std::path::Path;

use mapai_core::{LanguageId, Span, SymbolKind};
use mapai_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The Kotlin extractor. Registered by the CLI composition root (ADR-0003).
pub struct KotlinExtractor;

impl Extractor for KotlinExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("kotlin")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["kt", "kts"],
            shebangs: &[],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_kotlin_ng::LANGUAGE.into()
    }

    fn extract(&self, source: &[u8], tree: &Tree) -> Extraction {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        visit(tree.root_node(), source, false, &mut symbols, &mut imports);
        Extraction { symbols, imports }
    }

    fn resolve(
        &self,
        imports: &[RawImport],
        ctx: &dyn ResolutionContext,
        _config: &LangConfig,
    ) -> Vec<ResolvedImport> {
        let current_dir = parent_dir(&normalize(ctx.current_file()));
        let package = read_package(ctx.repo_root(), ctx.current_file()).unwrap_or_default();
        let root = source_root(&current_dir, &package);

        let mut resolved = Vec::new();
        for imp in imports {
            let spec = imp.specifier.as_str();
            if let Some(pkg) = spec.strip_suffix(".*") {
                let dir = join(&root, &pkg.replace('.', "/"));
                let files = ctx.files_in_dir(Path::new(&dir));
                if files.is_empty() {
                    resolved.push(ResolvedImport::External {
                        specifier: imp.specifier.clone(),
                    });
                } else {
                    for target in files {
                        resolved.push(ResolvedImport::Resolved {
                            target,
                            span: imp.span,
                        });
                    }
                }
            } else {
                let candidate = format!("{}.kt", join(&root, &spec.replace('.', "/")));
                match ctx.file_by_path(Path::new(&candidate)) {
                    Some(target) => resolved.push(ResolvedImport::Resolved {
                        target,
                        span: imp.span,
                    }),
                    None => resolved.push(ResolvedImport::External {
                        specifier: imp.specifier.clone(),
                    }),
                }
            }
        }
        resolved
    }
}

fn source_root(current_dir: &str, package: &str) -> String {
    let pkg_path = package.replace('.', "/");
    if !pkg_path.is_empty() && current_dir.ends_with(&pkg_path) {
        current_dir[..current_dir.len() - pkg_path.len()]
            .trim_end_matches('/')
            .to_string()
    } else {
        current_dir.to_string()
    }
}

fn join(root: &str, path: &str) -> String {
    if root.is_empty() {
        path.to_string()
    } else {
        format!("{root}/{path}")
    }
}

fn parent_dir(rel: &str) -> String {
    rel.rsplit_once('/')
        .map(|(d, _)| d)
        .unwrap_or("")
        .to_string()
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn read_package(repo_root: &Path, rel: &Path) -> Option<String> {
    let content = std::fs::read_to_string(repo_root.join(rel)).ok()?;
    for line in content.lines() {
        if let Some(rest) = line.trim().strip_prefix("package ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

fn visit(
    node: Node,
    src: &[u8],
    in_class: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    imports: &mut Vec<RawImport>,
) {
    match node.kind() {
        "class_declaration" => {
            // `class` / `interface` / `enum class` all parse as class_declaration; the
            // leading keyword (the text before the name) tells them apart.
            if let Some(name_node) = node.child_by_field_name("name") {
                let prefix = std::str::from_utf8(&src[node.start_byte()..name_node.start_byte()])
                    .unwrap_or("");
                let kind = if prefix.contains("interface") {
                    SymbolKind::Interface
                } else if prefix.contains("enum") {
                    SymbolKind::Enum
                } else {
                    SymbolKind::Class
                };
                if let Ok(name) = name_node.utf8_text(src) {
                    symbols.push(ExtractedSymbol {
                        name: name.to_string(),
                        kind,
                        span: span_of(name_node),
                    });
                }
            }
            recurse(node, src, true, symbols, imports);
            return;
        }
        "object_declaration" => {
            push_named(node, "name", SymbolKind::Class, src, symbols);
            recurse(node, src, true, symbols, imports);
            return;
        }
        "function_declaration" => {
            let kind = if in_class {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            push_named(node, "name", kind, src, symbols);
            recurse(node, src, false, symbols, imports);
            return;
        }
        "import" => {
            if let Some(qi) = first_child_of_kind(node, "qualified_identifier") {
                if let Ok(path) = qi.utf8_text(src) {
                    let specifier = if has_child_kind(node, "*") {
                        format!("{path}.*")
                    } else {
                        path.to_string()
                    };
                    imports.push(RawImport {
                        specifier,
                        span: span_of(node),
                    });
                }
            }
        }
        _ => {}
    }
    recurse(node, src, in_class, symbols, imports);
}

fn recurse(
    node: Node,
    src: &[u8],
    in_class: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    imports: &mut Vec<RawImport>,
) {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            visit(child, src, in_class, symbols, imports);
        }
        i += 1;
    }
}

fn has_child_kind(node: Node, kind: &str) -> bool {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            if child.kind() == kind {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn first_child_of_kind<'a>(node: Node<'a>, kind: &str) -> Option<Node<'a>> {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            if child.kind() == kind {
                return Some(child);
            }
        }
        i += 1;
    }
    None
}

fn push_named(
    node: Node,
    field: &str,
    kind: SymbolKind,
    src: &[u8],
    symbols: &mut Vec<ExtractedSymbol>,
) {
    if let Some(name_node) = node.child_by_field_name(field) {
        if let Ok(name) = name_node.utf8_text(src) {
            symbols.push(ExtractedSymbol {
                name: name.to_string(),
                kind,
                span: span_of(name_node),
            });
        }
    }
}

fn span_of(node: Node) -> Span {
    let start = node.start_position();
    Span {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_row: start.row,
        start_col: start.column,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
package com.example.app

import com.example.util.Helper

class Greeter {
    fun greet(): String { return "hi" }
}

interface Speaker {
    fun speak()
}

enum class Color { RED, GREEN }

object Singleton {
    fun run() {}
}

fun main() {
    println(Helper())
}
"#;

    #[test]
    fn extracts_symbols_and_imports() {
        let kt = KotlinExtractor;
        let grammar = kt.grammar();
        let tree = mapai_extract::parse(&grammar, SAMPLE.as_bytes()).expect("parse");
        let extraction = kt.extract(SAMPLE.as_bytes(), &tree);

        let by_name: Vec<(&str, SymbolKind)> = extraction
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        assert!(
            by_name.contains(&("Greeter", SymbolKind::Class)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Speaker", SymbolKind::Interface)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Color", SymbolKind::Enum)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("Singleton", SymbolKind::Class)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("greet", SymbolKind::Method)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("main", SymbolKind::Function)),
            "{by_name:?}"
        );

        let specs: Vec<&str> = extraction
            .imports
            .iter()
            .map(|i| i.specifier.as_str())
            .collect();
        assert!(specs.contains(&"com.example.util.Helper"), "{specs:?}");
    }

    #[test]
    fn source_root_strips_package() {
        assert_eq!(source_root("src/com/example/app", "com.example.app"), "src");
        assert_eq!(source_root("foo", ""), "foo");
    }
}
