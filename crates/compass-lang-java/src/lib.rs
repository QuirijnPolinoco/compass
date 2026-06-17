//! `compass-lang-java` — the Java language extractor.
//!
//! A self-contained unit behind the [`compass_extract::Extractor`] trait (ADR-0002):
//! detection, the tree-sitter-java grammar, symbol extraction (classes, interfaces,
//! enums, records, methods), and package/source-root-aware import resolution.

use std::path::Path;

use compass_core::{LanguageId, Span, SymbolKind};
use compass_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The Java extractor. Registered by the CLI composition root (ADR-0003).
pub struct JavaExtractor;

impl Extractor for JavaExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("java")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["java"],
            shebangs: &[],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_java::LANGUAGE.into()
    }

    fn extract(&self, source: &[u8], tree: &Tree) -> Extraction {
        let mut symbols = Vec::new();
        let mut imports = Vec::new();
        visit(tree.root_node(), source, &mut symbols, &mut imports);
        Extraction { symbols, imports }
    }

    fn resolve(
        &self,
        imports: &[RawImport],
        ctx: &dyn ResolutionContext,
        _config: &LangConfig,
    ) -> Vec<ResolvedImport> {
        // Java maps `package a.b.c` to a directory under a source root. Derive the source
        // root from the current file's package, then resolve each import's FQN under it.
        let current_dir = parent_dir(&normalize(ctx.current_file()));
        let package = read_package(ctx.repo_root(), ctx.current_file()).unwrap_or_default();
        let root = source_root(&current_dir, &package);

        let mut resolved = Vec::new();
        for imp in imports {
            let spec = imp.specifier.as_str();
            if let Some(pkg) = spec.strip_suffix(".*") {
                // Wildcard: depend on every file in the imported package directory.
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
                let candidate = format!("{}.java", join(&root, &spec.replace('.', "/")));
                match ctx.file_by_path(Path::new(&candidate)) {
                    // Not found ⇒ JDK / third-party (external), not a broken import.
                    None => resolved.push(ResolvedImport::External {
                        specifier: imp.specifier.clone(),
                    }),
                    Some(target) => resolved.push(ResolvedImport::Resolved {
                        target,
                        span: imp.span,
                    }),
                }
            }
        }
        resolved
    }
}

/// Strip the package path off the file's directory to find the Java source root, e.g.
/// dir `src/main/java/com/x` + package `com.x` -> `src/main/java`.
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
            return Some(rest.trim_end_matches(';').trim().to_string());
        }
    }
    None
}

fn visit(node: Node, src: &[u8], symbols: &mut Vec<ExtractedSymbol>, imports: &mut Vec<RawImport>) {
    match node.kind() {
        "class_declaration" => push_named(node, "name", SymbolKind::Class, src, symbols),
        "interface_declaration" => push_named(node, "name", SymbolKind::Interface, src, symbols),
        "enum_declaration" => push_named(node, "name", SymbolKind::Enum, src, symbols),
        "record_declaration" => push_named(node, "name", SymbolKind::Struct, src, symbols),
        "method_declaration" | "constructor_declaration" => {
            push_named(node, "name", SymbolKind::Method, src, symbols)
        }
        "import_declaration" => {
            let mut fqn: Option<String> = None;
            let mut wildcard = false;
            let mut i = 0usize;
            while i < node.child_count() {
                if let Some(child) = node.child(i as u32) {
                    match child.kind() {
                        "scoped_identifier" | "identifier" => {
                            if let Ok(text) = child.utf8_text(src) {
                                fqn = Some(text.to_string());
                            }
                        }
                        "asterisk" => wildcard = true,
                        _ => {}
                    }
                }
                i += 1;
            }
            if let Some(fqn) = fqn {
                let specifier = if wildcard { format!("{fqn}.*") } else { fqn };
                imports.push(RawImport {
                    specifier,
                    span: span_of(node),
                });
            }
        }
        _ => {}
    }

    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            visit(child, src, symbols, imports);
        }
        i += 1;
    }
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
    use compass_core::SymbolKind::{Class, Enum, Interface, Method, Struct};
    use compass_extract::testing::MockResolutionContext;
    use compass_extract::{LangConfig, RawImport, ResolvedImport};

    /// A rich sample exercising every symbol kind the extractor recognizes (class,
    /// constructor + methods, interface + its method, enum, record) and every import
    /// shape (single, wildcard, JDK, static).
    const SAMPLE: &str = r#"
package com.example.app;

import com.example.util.Helper;
import com.example.util.*;
import java.util.List;
import static java.lang.Math.PI;

public class Main {
    public Main() {}
    public void run() {}
    public static void main(String[] args) {}
}

interface Greeter {
    String greet();
}

enum Color { RED, GREEN }

record Point(int x, int y) {}
"#;

    fn extract(src: &str) -> Extraction {
        let x = JavaExtractor;
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
        // The record is a Struct; constructors and interface methods are Methods. The
        // constructor `Main` and the class `Main` share a name but differ in kind.
        let mut want = vec![
            ("Color".to_string(), Enum),
            ("Greeter".to_string(), Interface),
            ("Main".to_string(), Class),
            ("Main".to_string(), Method),
            ("Point".to_string(), Struct),
            ("greet".to_string(), Method),
            ("main".to_string(), Method),
            ("run".to_string(), Method),
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
        // Source order; the wildcard keeps its `.*` and the static import surfaces as the
        // full scoped name (the `static` modifier is dropped).
        assert_eq!(
            specs,
            [
                "com.example.util.Helper",
                "com.example.util.*",
                "java.util.List",
                "java.lang.Math.PI",
            ]
        );
    }

    #[test]
    fn resolve_classifies_internal_external_and_broken() {
        // resolve() re-reads the importing file for its `package` line, so `current`
        // writes it to disk. Source root = dir(current_file) minus the package path:
        // `src/com/example/app` minus `com/example/app` = `src`.
        let ctx = MockResolutionContext::new()
            .current(
                "src/com/example/app/Main.java",
                "package com.example.app;\n",
            )
            .file("src/com/example/util/Helper.java");

        let imports = [
            raw("com.example.util.Helper"),  // internal -> Resolved to Helper.java
            raw("java.util.List"),           // JDK, not mapped -> External
            raw("com.example.missing.Gone"), // internal-looking but not found -> External
            raw("com.example.util.*"),       // wildcard over a dir with one mapped file
        ];
        let resolved = JavaExtractor.resolve(&imports, &ctx, &LangConfig);

        // Exactly one Resolved per non-wildcard import + one per file in the wildcard dir.
        assert_eq!(resolved.len(), 4);

        match &resolved[0] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/com/example/util/Helper.java"))
            }
            other => panic!("internal import should resolve, got {other:?}"),
        }
        assert!(
            matches!(resolved[1], ResolvedImport::External { .. }),
            "JDK import is external, got {:?}",
            resolved[1]
        );
        // Java never emits Unresolved: a not-found import is treated as JDK/third-party.
        assert!(
            matches!(resolved[2], ResolvedImport::External { .. }),
            "not-found import is external (never broken), got {:?}",
            resolved[2]
        );
        match &resolved[3] {
            ResolvedImport::Resolved { target, .. } => {
                assert_eq!(*target, ctx.id_of("src/com/example/util/Helper.java"))
            }
            other => panic!("wildcard should resolve to files in the dir, got {other:?}"),
        }
    }

    #[test]
    fn wildcard_over_empty_dir_is_external() {
        // A wildcard whose package directory has no mapped files is external, not broken.
        let ctx = MockResolutionContext::new().current(
            "src/com/example/app/Main.java",
            "package com.example.app;\n",
        );
        let resolved = JavaExtractor.resolve(&[raw("com.example.nothing.*")], &ctx, &LangConfig);
        assert!(matches!(resolved[0], ResolvedImport::External { .. }));
    }

    #[test]
    fn source_root_strips_package() {
        assert_eq!(
            source_root("src/main/java/com/example/app", "com.example.app"),
            "src/main/java"
        );
        assert_eq!(source_root("com/example/app", "com.example.app"), "");
        assert_eq!(source_root("foo", ""), "foo");
    }
}
