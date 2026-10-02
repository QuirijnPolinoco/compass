//! Short string literals and configuration keys, language-agnostic.
//!
//! The value a task asks to change often lives in no identifier at all: a default answer list
//! (`['Yes', 'No', 'Maybe']`), a domain in a lookup table (`'twitter'`), a key in a settings
//! object (`qualityContent: { minEmojiCount }`). These words are weak evidence about which file
//! a task is about, so they are kept apart from symbols and ranked below them.
//!
//! Every grammar names its string literal nodes `*string*`, and object-literal keys are the
//! `key` of a `pair` (JavaScript/TypeScript) or the first child of a `keyed_element` (Go).

use std::collections::HashSet;

use tree_sitter::{Node, Tree};

/// Longest literal kept, in characters. Past this a string is prose, a query or a template,
/// whose words mostly describe something other than this file's own configuration.
const MAX_LITERAL_CHARS: usize = 60;

/// Most distinct literals kept per file, so a data-heavy file cannot flood the ranking.
const MAX_LITERALS: usize = 300;

/// The distinct short string literals and object-literal keys of a file, in source order.
pub fn file_literals(src: &[u8], tree: &Tree) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    collect(tree.root_node(), src, &mut seen, &mut out);
    out
}

fn collect(node: Node, src: &[u8], seen: &mut HashSet<String>, out: &mut Vec<String>) {
    if out.len() >= MAX_LITERALS {
        return;
    }
    let kind = node.kind();
    if kind.contains("comment") {
        return;
    }
    if kind.contains("string") {
        // The outermost string node holds the whole literal; its children are fragments.
        if let Some(text) = node.utf8_text(src).ok().and_then(unquote) {
            push(text, seen, out);
        }
        return;
    }
    let key = match kind {
        "pair" => node.child_by_field_name("key"),
        "keyed_element" => node.named_child(0),
        _ => None,
    };
    if let Some(key) = key.filter(|k| !k.kind().contains("string")) {
        if let Ok(text) = key.utf8_text(src) {
            push(text, seen, out);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect(child, src, seen, out);
    }
}

fn push(text: &str, seen: &mut HashSet<String>, out: &mut Vec<String>) {
    let text = text.trim();
    if text.chars().any(char::is_alphabetic) && seen.insert(text.to_string()) {
        out.push(text.to_string());
    }
}

/// A literal's text without its quotes and prefixes (`r"..."`, `b'...'`, `f"..."`, backticks),
/// or `None` when it spans lines or is too long to be a value someone would look up.
fn unquote(raw: &str) -> Option<&str> {
    if raw.contains('\n') {
        return None;
    }
    let body = raw
        .trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '@' || c == '$')
        .trim_matches(|c| matches!(c, '"' | '\'' | '`' | '#'));
    (!body.is_empty() && body.chars().count() <= MAX_LITERAL_CHARS).then_some(body)
}

#[cfg(test)]
mod tests {
    use super::unquote;

    #[test]
    fn literals_lose_their_quotes_and_prefixes() {
        assert_eq!(unquote("'Maybe'"), Some("Maybe"));
        assert_eq!(unquote("r#\"raw\"#"), Some("raw"));
        assert_eq!(unquote("f\"{name}\""), Some("{name}"));
        assert_eq!(unquote("`template`"), Some("template"));
        assert_eq!(unquote("\"\""), None);
        assert_eq!(unquote("\"a\nb\""), None);
        assert_eq!(unquote(&format!("\"{}\"", "x".repeat(61))), None);
    }
}
