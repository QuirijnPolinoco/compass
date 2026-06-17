//! `compass-lang-kotlin` — the Kotlin language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection (.kt/.kts), the `tree-sitter-kotlin-ng` grammar, symbol extraction (classes,
//! interfaces, enums, objects, functions/methods), and package-aware import resolution.
//!
//! Kotlin doesn't require a file's name to match its classes, so import resolution is a
//! best-effort convention (package mirrors folder; `import a.b.C` -> `a/b/C.kt` under the
//! source root). Unresolved imports are treated as external, never flagged broken.

use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
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
    use compass_core::SymbolKind::{Class, Enum, Function, Interface, Method};
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    // A rich sample exercising every symbol kind the extractor distinguishes
    // (class / interface / enum / object / top-level fn / member fn) plus imports
    // of every shape it recognizes: internal single-symbol, wildcard, stdlib, and
    // third-party.
    const SAMPLE: &str = r#"
package com.example.app

import com.example.util.Helper
import com.example.util.Logger
import com.example.model.*
import kotlin.collections.List
import com.acme.thirdparty.Widget

class Greeter {
    fun greet(): String { return "hi" }
    fun shout(): String { return "HI" }
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

fun helper() {}
"#;

    fn extract(src: &str) -> Extraction {
        let x = KotlinExtractor;
        let tree = compass_extract::parse(&x.grammar(), src.as_bytes()).expect("parse");
        x.extract(src.as_bytes(), &tree)
    }

    fn raw(specifier: &str) -> RawImport {
        RawImport {
            specifier: specifier.to_string(),
            span: Span {
                start_byte: 0,
                end_byte: 0,
                start_row: 0,
                start_col: 0,
            },
        }
    }

    #[test]
    fn extracts_exactly_the_expected_symbols() {
        let mut got: Vec<(String, SymbolKind)> = extract(SAMPLE)
            .symbols
            .into_iter()
            .map(|s| (s.name, s.kind))
            .collect();
        got.sort();
        // Note: enum entries (RED, GREEN) are not declarations, so they are not
        // extracted; `object` declarations map to Class; functions are Method inside
        // a class/interface/object and Function at top level.
        let mut want = vec![
            ("Color".to_string(), Enum),
            ("Greeter".to_string(), Class),
            ("Singleton".to_string(), Class),
            ("Speaker".to_string(), Interface),
            ("greet".to_string(), Method),
            ("helper".to_string(), Function),
            ("main".to_string(), Function),
            ("run".to_string(), Method),
            ("shout".to_string(), Method),
            ("speak".to_string(), Method),
        ];
        want.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn extracts_all_imports_in_order() {
        let specs: Vec<String> = extract(SAMPLE)
            .imports
            .into_iter()
            .map(|i| i.specifier)
            .collect();
        assert_eq!(
            specs,
            [
                "com.example.util.Helper",
                "com.example.util.Logger",
                "com.example.model.*",
                "kotlin.collections.List",
                "com.acme.thirdparty.Widget",
            ]
        );
    }

    #[test]
    fn resolve_classifies_internal_external_and_broken() {
        // resolve() re-reads the current file for its `package` line to find the source
        // root: with `package com.example.app`, the dir `src/com/example/app` yields
        // source root `src`, so `import a.b.C` maps to `src/a/b/C.kt`.
        let ctx = MockResolutionContext::new()
            .current("src/com/example/app/Main.kt", "package com.example.app\n")
            .file("src/com/example/util/Helper.kt");
        let imports = [
            raw("com.example.util.Helper"),  // internal -> Resolved
            raw("kotlin.collections.List"),  // stdlib -> External
            raw("com.example.util.Missing"), // internal-looking but absent -> External
        ];
        let resolved = KotlinExtractor.resolve(&imports, &ctx, &LangConfig);

        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/com/example/util/Helper.kt"))
            }
            other => panic!("internal import should resolve, got {other:?}"),
        }
        assert!(
            matches!(resolved[1], ResolvedImport::External { .. }),
            "stdlib import is external, got {:?}",
            resolved[1]
        );
        // Kotlin never flags broken imports: an unresolved-but-internal-looking import
        // is treated as External, never Unresolved (see module docs).
        assert!(
            matches!(resolved[2], ResolvedImport::External { .. }),
            "absent target is treated as external (never Unresolved), got {:?}",
            resolved[2]
        );
        assert!(
            !resolved
                .iter()
                .any(|r| matches!(r, ResolvedImport::Unresolved { .. })),
            "this language produces no Unresolved imports"
        );
    }

    #[test]
    fn resolve_handles_wildcard_imports() {
        // `import a.b.*` resolves to every mapped file in dir `src/a/b`; an empty dir
        // falls back to External.
        let ctx = MockResolutionContext::new()
            .current("src/com/example/app/Main.kt", "package com.example.app\n")
            .file("src/com/example/model/User.kt")
            .file("src/com/example/model/Order.kt");
        let imports = [
            raw("com.example.model.*"), // expands to the two mapped files
            raw("com.example.empty.*"), // no files in dir -> External
        ];
        let resolved = KotlinExtractor.resolve(&imports, &ctx, &LangConfig);

        let resolved_targets: Vec<_> = resolved
            .iter()
            .filter_map(|r| match r {
                ResolvedImport::Resolved { target, .. } => Some(*target),
                _ => None,
            })
            .collect();
        assert_eq!(resolved_targets.len(), 2, "wildcard expands to both files");
        assert!(resolved_targets.contains(&ctx.id_of("src/com/example/model/User.kt")));
        assert!(resolved_targets.contains(&ctx.id_of("src/com/example/model/Order.kt")));
        assert!(
            matches!(resolved.last(), Some(ResolvedImport::External { .. })),
            "empty wildcard dir is external"
        );
    }

    #[test]
    fn source_root_strips_package() {
        assert_eq!(source_root("src/com/example/app", "com.example.app"), "src");
        assert_eq!(source_root("foo", ""), "foo");
    }
}
