//! Doc-comment summaries, language-agnostic.
//!
//! The words a person uses to describe code ("the real-money switch") often appear nowhere in
//! its identifiers (`LiveEnablePanel`), but they are usually in the comment written above it.
//! This module reads those comments for any tree-sitter grammar, so no extractor has to: every
//! grammar names its comment nodes `*comment*`, and attributes and decorators sit on their own
//! lines. Python docstrings, the one common doc form that is not a comment, are read from the
//! first statement of a body.
//!
//! Only a summary is kept: the first sentence of the first paragraph, markers and markup
//! stripped, capped in length. It feeds ranking and is short enough to show an agent.

use compass_extract::ExtractedSymbol;
use tree_sitter::{Node, Tree};

/// Longest summary kept, in characters. A first sentence is rarely longer; past this it is
/// prose about details, not a description.
const MAX_SUMMARY_CHARS: usize = 160;

/// How far down a file its summary comment may start, in lines: room for a shebang, an encoding
/// line or a license header above it.
const SUMMARY_SEARCH_LINES: usize = 40;

/// The summaries found in one file.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FileDocs {
    /// What the file as a whole is for: its leading doc comment or module docstring.
    pub summary: Option<String>,
    /// One entry per extracted symbol, in the same order.
    pub symbols: Vec<Option<String>>,
}

/// One comment, by the lines it covers.
struct Comment {
    start_row: usize,
    end_row: usize,
    text: String,
}

/// Read the doc summaries for `symbols` (as extracted from `src`) and for the file itself.
pub fn file_docs(src: &[u8], tree: &Tree, symbols: &[ExtractedSymbol]) -> FileDocs {
    let text = String::from_utf8_lossy(src);
    let lines: Vec<&str> = text.lines().collect();
    let mut comments = Vec::new();
    collect_comments(tree.root_node(), src, &mut comments);

    let symbols = symbols
        .iter()
        .map(|s| {
            comment_above(s.span.start_row, &comments, &lines)
                .or_else(|| docstring_of(tree, s.span.start_byte, s.span.end_byte, src))
                .and_then(|raw| summarize(&raw))
        })
        .collect();
    FileDocs {
        summary: file_summary(tree, src, &comments, &lines),
        symbols,
    }
}

fn collect_comments(node: Node, src: &[u8], out: &mut Vec<Comment>) {
    if node.kind().contains("comment") {
        if let Ok(text) = node.utf8_text(src) {
            let (start, end) = (node.start_position(), node.end_position());
            // Some grammars end a line comment at column 0 of the next row (its newline).
            let end_row = if end.column == 0 && end.row > start.row {
                end.row - 1
            } else {
                end.row
            };
            out.push(Comment {
                start_row: start.row,
                end_row,
                text: text.trim_end().to_string(),
            });
        }
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_comments(child, src, out);
    }
}

/// The comment block that ends right above `row`, read upward through adjacent comments and
/// over attribute/decorator lines. A blank line or code ends the block.
fn comment_above(row: usize, comments: &[Comment], lines: &[&str]) -> Option<String> {
    let mut r = row;
    // The definition may start above the name's line (`export`, a modifier on its own line is
    // rare; attributes are common): skip attribute lines first.
    while r > 0 && is_attribute_line(lines.get(r - 1).copied().unwrap_or_default()) {
        r -= 1;
    }
    let mut block: Vec<&str> = Vec::new();
    while r > 0 {
        let Some(c) = comments.iter().rev().find(|c| c.end_row == r - 1) else {
            break;
        };
        // A trailing comment after code on the same line documents that line, not ours.
        let line = lines.get(c.start_row).copied().unwrap_or_default();
        if !line
            .trim_start()
            .starts_with(c.text.lines().next().unwrap_or_default().trim())
        {
            break;
        }
        block.push(&c.text);
        r = c.start_row;
        while r > 0 && is_attribute_line(lines.get(r - 1).copied().unwrap_or_default()) {
            r -= 1;
        }
    }
    if block.is_empty() {
        return None;
    }
    block.reverse();
    Some(block.join("\n"))
}

/// `#[derive(..)]`, `@Override`, `@dataclass`, `[Serializable]`: lines between a doc comment and
/// the item it documents.
fn is_attribute_line(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("#[") || t.starts_with('@') || (t.starts_with('[') && t.trim_end().ends_with(']'))
}

/// A Python-style docstring: the body's first statement, when it is a bare string.
fn docstring_of(tree: &Tree, start: usize, end: usize, src: &[u8]) -> Option<String> {
    let name = tree.root_node().descendant_for_byte_range(start, end)?;
    let body = name.parent()?.child_by_field_name("body")?;
    leading_string(body, src)
}

/// The text of `body`'s first statement (comments aside) if it is a string expression.
fn leading_string(body: Node, src: &[u8]) -> Option<String> {
    let mut cursor = body.walk();
    let first = body
        .named_children(&mut cursor)
        .find(|n| !n.kind().contains("comment"))?;
    let string = if first.kind() == "expression_statement" {
        first.named_child(0)?
    } else {
        first
    };
    // Triple-quoted only: a single-quoted leading string is a directive (`"use strict"`).
    let text = string.utf8_text(src).ok()?;
    let quoted = text.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    (string.kind() == "string" && (quoted.starts_with("\"\"\"") || quoted.starts_with("'''")))
        .then(|| text.to_string())
}

/// The file's own description: a module docstring, or the first comment block near the top
/// that is not a license and is not simply the doc of the first item below it.
fn file_summary(tree: &Tree, src: &[u8], comments: &[Comment], lines: &[&str]) -> Option<String> {
    if let Some(doc) = leading_string(tree.root_node(), src) {
        return summarize(&doc);
    }
    let mut i = 0;
    while i < comments.len() && comments[i].start_row < SUMMARY_SEARCH_LINES {
        // Merge adjacent comments into one block.
        let start = i;
        while i + 1 < comments.len() && comments[i + 1].start_row == comments[i].end_row + 1 {
            i += 1;
        }
        let block: Vec<&str> = comments[start..=i]
            .iter()
            .map(|c| c.text.as_str())
            .collect();
        let joined = block.join("\n");
        let end_row = comments[i].end_row;
        i += 1;

        let first = block[0].trim_start();
        if first.starts_with("#!") || is_magic_comment(first) || looks_like_license(&joined) {
            continue;
        }
        // Code above it (anything but blank lines and the comments already passed over): not a
        // header.
        let in_comment = |row: usize| {
            comments[..start]
                .iter()
                .any(|c| (c.start_row..=c.end_row).contains(&row))
        };
        if (0..comments[start].start_row)
            .any(|row| !lines.get(row).map_or("", |l| l.trim()).is_empty() && !in_comment(row))
        {
            return None;
        }
        let inner = first.starts_with("//!") || first.starts_with("/*!");
        let next = lines.get(end_row + 1).map_or("", |l| l.trim());
        let documents_next_item = !next.is_empty() && !is_namespace_line(next);
        if inner || !documents_next_item {
            return summarize(&joined);
        }
        return None;
    }
    None
}

/// `package x`, `namespace X`, `module X`: a declaration a file-level comment sits right above.
fn is_namespace_line(line: &str) -> bool {
    ["package ", "namespace ", "module ", "<?php"]
        .iter()
        .any(|p| line.starts_with(p))
}

/// `# -*- coding: utf-8 -*-`, `# frozen_string_literal: true`, `// @ts-nocheck`, `"use strict"`.
fn is_magic_comment(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("coding:")
        || t.contains("frozen_string_literal")
        || t.contains("@ts-")
        || t.contains("eslint-")
}

fn looks_like_license(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("copyright") || t.contains("spdx-license") || t.contains("licensed under")
}

/// The first sentence of a raw comment or docstring, with comment markers, doc tags and markup
/// removed. `None` when nothing descriptive is left.
pub fn summarize(raw: &str) -> Option<String> {
    let mut words: Vec<String> = Vec::new();
    for line in raw.lines() {
        let line = strip_markers(line);
        if line.is_empty() {
            if words.is_empty() {
                continue;
            }
            break; // end of the first paragraph
        }
        // `@param x`, `:param x:`, `<param name="x">`: details, not the summary.
        if line.starts_with('@') || line.starts_with(":param") || line.starts_with(":return") {
            break;
        }
        words.extend(strip_xml(line).split_whitespace().map(str::to_string));
    }
    let text = words.join(" ");
    let sentence = match text.find(". ") {
        Some(end) => &text[..=end],
        None => &text,
    };
    let sentence = sentence.trim();
    if sentence.chars().filter(|c| c.is_alphabetic()).count() < 3 {
        return None;
    }
    if sentence.chars().count() <= MAX_SUMMARY_CHARS {
        return Some(sentence.to_string());
    }
    let cut: String = sentence.chars().take(MAX_SUMMARY_CHARS).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    Some(format!("{cut}..."))
}

/// One comment line without its markers: `///`, `//!`, `//`, `/**`, `*/`, a leading `*`, `#`,
/// `--`, triple quotes.
fn strip_markers(line: &str) -> &str {
    let mut t = line.trim();
    for prefix in [
        "/**", "/*!", "/*", "///", "//!", "//", "#", "--", "\"\"\"", "'''", "*",
    ] {
        if let Some(rest) = t.strip_prefix(prefix) {
            t = rest.trim_start();
            break;
        }
    }
    for suffix in ["*/", "\"\"\"", "'''"] {
        if let Some(rest) = t.strip_suffix(suffix) {
            t = rest.trim_end();
        }
    }
    t.trim()
}

/// Drop XML doc tags (`<summary>`, `<see cref="X"/>`) but keep their text.
fn strip_xml(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_tag = false;
    for c in line.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::summarize;

    #[test]
    fn summaries_keep_the_first_sentence_without_markers_or_tags() {
        assert_eq!(
            summarize("/// Applies the risk limits.\n/// Details follow here.").as_deref(),
            Some("Applies the risk limits.")
        );
        assert_eq!(
            summarize(
                "/**\n * The deliberate real-money switch. Flipping it\n * ON places orders.\n */"
            )
            .as_deref(),
            Some("The deliberate real-money switch.")
        );
        assert_eq!(
            summarize("/// <summary>\n/// Signs a request.\n/// </summary>").as_deref(),
            Some("Signs a request.")
        );
        assert_eq!(
            summarize("\"\"\"Load the config.\n\n    More text.\n    \"\"\"").as_deref(),
            Some("Load the config.")
        );
        assert_eq!(
            summarize("/**\n * @param x the value\n */"),
            None,
            "tags alone are not a summary"
        );
        assert_eq!(summarize("// ---------------"), None);
    }

    #[test]
    fn long_summaries_are_cut_at_a_word() {
        let long = format!("// {}", "word ".repeat(60));
        let s = summarize(&long).unwrap();
        assert!(s.ends_with("...") && s.chars().count() <= 163, "{s}");
    }
}
