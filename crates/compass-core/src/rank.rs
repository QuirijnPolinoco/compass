//! Ranking files against free text, for `context --query` (ADR-0006 pre-injection).
//!
//! A prompt is natural language ("raise the medium-risk drawdown cap"), while the map holds paths
//! and identifiers (`risk-gate/src/gate.rs`, `medium_risk`, `max_drawdown_pct_of_equity`). Both
//! sides are therefore reduced to the same vocabulary: identifiers are split into their words
//! (snake/kebab/camel case, path segments), words are lightly stemmed, and English filler is
//! dropped. A term then matches a word when they are equal or one is a prefix of the other
//! (`reject` ~ `rejection`, `sign` ~ `signature`), never a bare substring, so `cap` no longer
//! matches `escape` and `the` no longer matches `theme`.
//!
//! Terms are weighted by inverse document frequency: a word that appears in a handful of files
//! (`drawdown`) says far more about the target than one that appears everywhere (`risk` in a
//! risk-engine repo), so it should decide the ranking.

use std::collections::HashSet;

/// English filler that carries no signal about *which file* a task is about. Words shorter than
/// three characters are dropped before this list is consulted.
const STOPWORDS: &[&str] = &[
    "about", "above", "add", "after", "all", "also", "and", "any", "are", "because", "been",
    "before", "being", "but", "can", "change", "code", "could", "does", "doing", "done", "each",
    "file", "files", "find", "fix", "for", "from", "get", "has", "have", "how", "into", "its",
    "just", "like", "make", "many", "more", "most", "much", "need", "new", "not", "now", "only",
    "other", "our", "out", "over", "please", "same", "should", "show", "some", "such", "than",
    "that", "the", "their", "them", "then", "there", "these", "they", "this", "those", "too",
    "use", "used", "uses", "using", "want", "was", "way", "were", "what", "when", "where", "which",
    "while", "who", "why", "will", "with", "would", "you", "your",
];

/// Minimum length for prefix matching; shorter words must match exactly.
const MIN_PREFIX: usize = 4;

/// The distinct, stemmed, non-filler terms of a free-text query, in order of first appearance.
pub(crate) fn query_terms(query: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    split_words(query)
        .into_iter()
        .filter(|w| w.len() >= 3 && !w.chars().all(|c| c.is_ascii_digit()))
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
        .map(|w| stem(&w))
        .filter(|w| seen.insert(w.clone()))
        .collect()
}

/// The stemmed words of an identifier or path (`max_drawdown_pct`, `LiveEnablePanel.tsx`,
/// `risk-gate/src/gate.rs`).
pub(crate) fn identifier_words(ident: &str) -> Vec<String> {
    split_words(ident)
        .into_iter()
        .filter(|w| !w.is_empty())
        .map(|w| stem(&w))
        .collect()
}

/// Whether a query term matches an identifier word: equal, or (for words long enough to be
/// meaningful) one is a prefix of the other.
pub(crate) fn term_matches(term: &str, word: &str) -> bool {
    term == word
        || (term.len() >= MIN_PREFIX
            && word.len() >= MIN_PREFIX
            && (word.starts_with(term) || term.starts_with(word)))
}

/// Whether any of `words` matches `term`.
pub(crate) fn any_match(term: &str, words: &[String]) -> bool {
    words.iter().any(|w| term_matches(term, w))
}

/// Words beyond which an identifier reads as a sentence. Names that long are almost always test
/// cases (`medium_risk_tiny_cell_skips_cleanly`), which live inline in source files in Rust,
/// Go and others: they match many query words by describing behaviour, not by implementing it.
const SENTENCE_WORDS: usize = 5;

/// How much a symbol's name counts as evidence: 1 for an ordinary identifier, less for a
/// sentence-like (test-case) name.
pub(crate) fn symbol_weight(words: &[String]) -> f64 {
    if words.len() > SENTENCE_WORDS {
        0.3
    } else {
        1.0
    }
}

/// Whether the query is about styling or page structure, so CSS and HTML are the target rather
/// than noise (their selectors match everyday words like `panel` and `risk`).
pub(crate) fn about_markup(terms: &[String]) -> bool {
    terms.iter().any(|t| {
        matches!(
            t.as_str(),
            "css"
                | "html"
                | "style"
                | "styl"
                | "stylesheet"
                | "layout"
                | "color"
                | "colour"
                | "theme"
                | "font"
                | "margin"
                | "padding"
                | "selector"
                | "markup"
        )
    })
}

/// Whether a repo-relative path looks like a test, by the conventions shared across languages
/// (a `test`/`tests`/`__tests__`/`spec` directory, or a `_test`, `.test`, `.spec`, `test_` name).
pub(crate) fn is_test_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    let mut segments: Vec<&str> = lower.split('/').collect();
    let name = segments.pop().unwrap_or_default();
    if segments
        .iter()
        .any(|s| matches!(*s, "test" | "tests" | "__tests__" | "spec" | "specs"))
    {
        return true;
    }
    let stem = name.split('.').next().unwrap_or_default();
    stem == "tests"
        || stem == "test"
        || stem.starts_with("test_")
        || stem.ends_with("_test")
        || stem.ends_with("_tests")
        || name.contains(".test.")
        || name.contains(".spec.")
}

/// Lowercased words of `text`, split on non-alphanumerics and on camelCase boundaries
/// (`LiveEnablePanel` gives live, enable, panel; `HTTPServer` gives http, server).
fn split_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    for chunk in text.split(|c: char| !c.is_alphanumeric()) {
        let chars: Vec<char> = chunk.chars().collect();
        let mut current = String::new();
        for (i, &c) in chars.iter().enumerate() {
            let boundary = i > 0 && c.is_uppercase() && {
                let prev = chars[i - 1];
                prev.is_lowercase()
                    || prev.is_ascii_digit()
                    || (prev.is_uppercase() && chars.get(i + 1).is_some_and(|n| n.is_lowercase()))
            };
            if boundary && !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            current.extend(c.to_lowercase());
        }
        if !current.is_empty() {
            words.push(current);
        }
    }
    words
}

/// A deliberately light English stemmer: strip one common inflectional suffix when enough of the
/// word remains. Over-stemming is cheap here because matching is prefix-tolerant anyway; the goal
/// is only that `rejected`, `rejection` and `rejects` meet at `reject`.
fn stem(word: &str) -> String {
    const SUFFIXES: [&str; 8] = ["ations", "ation", "ions", "ion", "ings", "ing", "ed", "es"];
    for suffix in SUFFIXES {
        if let Some(base) = word.strip_suffix(suffix) {
            if base.len() >= MIN_PREFIX {
                return base.to_string();
            }
        }
    }
    match word.strip_suffix('s') {
        Some(base) if base.len() >= MIN_PREFIX && !base.ends_with('s') => base.to_string(),
        _ => word.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_terms_drop_filler_digits_and_short_words_and_stem() {
        assert_eq!(
            query_terms("Raise the medium-risk profile's maximum drawdown cap from 15% to 18%."),
            vec!["raise", "medium", "risk", "profile", "maximum", "drawdown", "cap"]
        );
        assert_eq!(
            query_terms("too many orders get rejected"),
            vec!["order", "reject"]
        );
    }

    #[test]
    fn identifiers_split_on_case_and_separators() {
        assert_eq!(
            identifier_words("LiveEnablePanel.tsx"),
            vec!["live", "enable", "panel", "tsx"]
        );
        assert_eq!(identifier_words("HTTPServer"), vec!["http", "server"]);
        assert_eq!(
            identifier_words("risk-gate/src/max_drawdown_pct"),
            vec!["risk", "gate", "src", "max", "drawdown", "pct"]
        );
    }

    #[test]
    fn matching_is_by_word_and_prefix_never_by_substring() {
        assert!(term_matches("reject", "reject"));
        assert!(term_matches("sign", "signature"));
        assert!(term_matches("config", "configur"));
        // Short words need an exact match, and a word inside another word is not a match.
        assert!(!term_matches("cap", "capture"));
        assert!(!term_matches("cap", "escape"));
        assert!(!term_matches("risk", "brisk"));
    }

    #[test]
    fn test_paths_are_recognised_across_conventions() {
        for p in [
            "crates/risk-gate/src/tests.rs",
            "crates/web/tests/auth.rs",
            "src/__tests__/api.ts",
            "pkg/server_test.go",
            "tests/test_models.py",
            "app/src/Panel.test.tsx",
            "app/src/panel.spec.ts",
        ] {
            assert!(is_test_path(p), "{p}");
        }
        for p in [
            "crates/risk-gate/src/gate.rs",
            "src/attestation.rs",
            "src/latest.ts",
        ] {
            assert!(!is_test_path(p), "{p}");
        }
    }
}
