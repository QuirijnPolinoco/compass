//! Small helpers over tree-sitter-json nodes.

use compass_core::Span;
use tree_sitter::Node;

/// The `(key, key node, value node)` members of `object`, in source order. Members the parser
/// could not make sense of (no key, no value) are skipped.
pub(crate) fn pairs<'t>(object: Node<'t>, src: &[u8]) -> Vec<(String, Node<'t>, Node<'t>)> {
    let mut out = Vec::new();
    if object.kind() != "object" {
        return out;
    }
    let mut i = 0usize;
    while i < object.named_child_count() {
        if let Some(pair) = object.named_child(i as u32).filter(|p| p.kind() == "pair") {
            let key = pair.child_by_field_name("key");
            let value = pair.child_by_field_name("value");
            if let (Some(key), Some(value)) = (key, value) {
                if let Some(text) = string_text(key, src) {
                    out.push((text, key, value));
                }
            }
        }
        i += 1;
    }
    out
}

/// The value of `object[key]`, if `object` is one and has it.
pub(crate) fn get<'t>(object: Node<'t>, key: &str, src: &[u8]) -> Option<Node<'t>> {
    pairs(object, src)
        .into_iter()
        .find(|(k, _, _)| k == key)
        .map(|(_, _, value)| value)
}

/// The contents of a `string` node, without its quotes. Escapes are left as written: these are
/// names to look up, and a name with an escape in it is looked up the way it is spelled.
pub(crate) fn string_text(string: Node, src: &[u8]) -> Option<String> {
    if string.kind() != "string" {
        return None;
    }
    let raw = string.utf8_text(src).ok()?;
    Some(raw.trim_matches('"').to_string())
}

pub(crate) fn span_of(node: Node) -> Span {
    let start = node.start_position();
    Span {
        start_byte: node.start_byte(),
        end_byte: node.end_byte(),
        start_row: start.row,
        start_col: start.column,
    }
}
