//! `mapai-lang-python` — the Python language extractor.
//!
//! A self-contained unit behind the [`mapai_extract::Extractor`] trait (ADR-0002):
//! detection, the tree-sitter-python grammar, symbol extraction (functions, classes,
//! methods), and Python's relative + absolute import resolution.

use std::path::Path;

use mapai_core::{LanguageId, Span, SymbolKind};
use mapai_extract::{
    Detection, ExtractedSymbol, Extraction, Extractor, LangConfig, RawImport, ResolutionContext,
    ResolvedImport,
};
use tree_sitter::{Language, Node, Tree};

/// The Python extractor. Registered by the CLI composition root (ADR-0003).
pub struct PythonExtractor;

impl Extractor for PythonExtractor {
    fn language_id(&self) -> LanguageId {
        LanguageId::new("python")
    }

    fn detection(&self) -> Detection {
        Detection {
            extensions: &["py", "pyi"],
            shebangs: &["python"],
        }
    }

    fn grammar(&self) -> Language {
        tree_sitter_python::LANGUAGE.into()
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
        let current = normalize(ctx.current_file());
        let current_dir = current.rsplit_once('/').map(|(d, _)| d).unwrap_or("");

        imports
            .iter()
            .map(|imp| {
                let spec = imp.specifier.as_str();
                let relative = spec.starts_with('.');
                let candidates = if relative {
                    relative_candidates(current_dir, spec)
                } else {
                    absolute_candidates(spec)
                };

                for cand in &candidates {
                    if let Some(target) = ctx.file_by_path(Path::new(cand)) {
                        return ResolvedImport::Resolved {
                            target,
                            span: imp.span,
                        };
                    }
                }

                if relative {
                    // Relative imports must resolve inside the repo; if not, it's broken.
                    ResolvedImport::Unresolved {
                        specifier: imp.specifier.clone(),
                        span: imp.span,
                        reason: "relative import resolves to no file in the repo".to_string(),
                    }
                } else {
                    // Absolute imports that aren't in-repo are stdlib / third-party.
                    ResolvedImport::External {
                        specifier: imp.specifier.clone(),
                    }
                }
            })
            .collect()
    }
}

/// Candidate repo-relative files for an absolute module path (`a.b.c`).
fn absolute_candidates(module: &str) -> Vec<String> {
    let base = module.replace('.', "/");
    vec![format!("{base}.py"), format!("{base}/__init__.py")]
}

/// Candidate repo-relative files for a relative import (`.mod`, `..pkg.sub`, `.`).
fn relative_candidates(current_dir: &str, spec: &str) -> Vec<String> {
    let dots = spec.chars().take_while(|&c| c == '.').count();
    let rest = &spec[dots..];
    // 1 leading dot = current package; each extra dot goes one level up.
    let base = go_up(current_dir, dots.saturating_sub(1));
    let sub = rest.replace('.', "/");

    let joined = match (base.is_empty(), sub.is_empty()) {
        (true, _) => sub.clone(),
        (false, true) => base.clone(),
        (false, false) => format!("{base}/{sub}"),
    };

    if rest.is_empty() {
        if joined.is_empty() {
            vec!["__init__.py".to_string()]
        } else {
            vec![format!("{joined}/__init__.py")]
        }
    } else {
        vec![format!("{joined}.py"), format!("{joined}/__init__.py")]
    }
}

/// Drop `levels` trailing components from a `/`-separated directory path.
fn go_up(dir: &str, levels: usize) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for _ in 0..levels {
        parts.pop();
    }
    parts.join("/")
}

fn normalize(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Recursively pull symbols and imports. `in_class` makes a `def` a Method rather than a
/// Function.
fn visit(
    node: Node,
    src: &[u8],
    in_class: bool,
    symbols: &mut Vec<ExtractedSymbol>,
    imports: &mut Vec<RawImport>,
) {
    match node.kind() {
        "function_definition" => {
            let kind = if in_class {
                SymbolKind::Method
            } else {
                SymbolKind::Function
            };
            push_named(node, "name", kind, src, symbols);
            // A function body's own defs are plain functions, not methods.
            recurse(node, src, false, symbols, imports);
            return;
        }
        "class_definition" => {
            push_named(node, "name", SymbolKind::Class, src, symbols);
            recurse(node, src, true, symbols, imports);
            return;
        }
        "import_statement" => extract_plain_imports(node, src, imports),
        "import_from_statement" => {
            if let Some(module) = node.child_by_field_name("module_name") {
                if let Ok(text) = module.utf8_text(src) {
                    imports.push(RawImport {
                        specifier: text.to_string(),
                        span: span_of(module),
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

/// `import a.b`, `import a.b as c` — record each module's dotted path.
fn extract_plain_imports(node: Node, src: &[u8], imports: &mut Vec<RawImport>) {
    let mut i = 0usize;
    while i < node.child_count() {
        if let Some(child) = node.child(i as u32) {
            let name_node = match child.kind() {
                "dotted_name" => Some(child),
                "aliased_import" => child.child_by_field_name("name"),
                _ => None,
            };
            if let Some(n) = name_node {
                if let Ok(text) = n.utf8_text(src) {
                    imports.push(RawImport {
                        specifier: text.to_string(),
                        span: span_of(n),
                    });
                }
            }
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

    const SAMPLE: &str = r#"
import os
import a.b as ab
from pkg.mod_a import helper
from . import sibling
from ..util import thing

class Greeter:
    def greet(self):
        def inner():
            return 1
        return inner()

def main():
    pass
"#;

    #[test]
    fn extracts_symbols_and_imports() {
        let py = PythonExtractor;
        let grammar = py.grammar();
        let tree = mapai_extract::parse(&grammar, SAMPLE.as_bytes()).expect("parse");
        let extraction = py.extract(SAMPLE.as_bytes(), &tree);

        let mut by_name: Vec<(&str, SymbolKind)> = extraction
            .symbols
            .iter()
            .map(|s| (s.name.as_str(), s.kind))
            .collect();
        by_name.sort_by_key(|(n, _)| *n);

        assert!(
            by_name.contains(&("Greeter", SymbolKind::Class)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("greet", SymbolKind::Method)),
            "{by_name:?}"
        );
        assert!(
            by_name.contains(&("inner", SymbolKind::Function)),
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
        assert!(specs.contains(&"os"), "{specs:?}");
        assert!(specs.contains(&"a.b"), "{specs:?}");
        assert!(specs.contains(&"pkg.mod_a"), "{specs:?}");
        assert!(specs.contains(&"."), "{specs:?}");
        assert!(specs.contains(&"..util"), "{specs:?}");
    }

    #[test]
    fn absolute_candidates_cover_module_and_package() {
        assert_eq!(
            absolute_candidates("pkg.mod_a"),
            vec![
                "pkg/mod_a.py".to_string(),
                "pkg/mod_a/__init__.py".to_string()
            ]
        );
    }

    #[test]
    fn relative_candidates_walk_up() {
        // From `app/sub/main.py`, `.mod` is a sibling in `app/sub`.
        assert_eq!(
            relative_candidates("app/sub", ".mod"),
            vec![
                "app/sub/mod.py".to_string(),
                "app/sub/mod/__init__.py".to_string()
            ]
        );
        // `..util` goes up one level to `app/util`.
        assert_eq!(
            relative_candidates("app/sub", "..util"),
            vec![
                "app/util.py".to_string(),
                "app/util/__init__.py".to_string()
            ]
        );
        // `from . import x` points at the current package's __init__.
        assert_eq!(
            relative_candidates("app/sub", "."),
            vec!["app/sub/__init__.py".to_string()]
        );
    }
}
