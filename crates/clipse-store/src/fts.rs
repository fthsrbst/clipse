//! Full-text search: what gets indexed, and how to turn arbitrary user input
//! into a safe FTS5 `MATCH` expression.

use clipse_core::Clip;

/// The searchable text for one clip: its preview plus the full `text/plain`
/// payload when there is one. The preview alone is not enough — it is
/// truncated at a few hundred characters (see `clip::PREVIEW_MAX_CHARS`), so
/// a word past that cutoff would otherwise be unfindable.
pub(crate) fn body_text(clip: &Clip) -> String {
    match clip.text() {
        Some(text) if text != clip.preview => format!("{}\n{}", clip.preview, text),
        _ => clip.preview.clone(),
    }
}

/// Turns free-form user input into an FTS5 `MATCH` argument that can never be
/// interpreted as query syntax.
///
/// FTS5's default query grammar treats bare words as operators (`AND`, `OR`,
/// `NOT`, `NEAR`), a trailing `*` as a prefix wildcard, and an unbalanced `"`
/// as a syntax error. A peer- or user-supplied search string must not be able
/// to trigger any of that, so every whitespace-separated token is wrapped in
/// its own double-quoted string literal (FTS5's escape for an embedded `"` is
/// doubling it, same as SQL string literals). Quoted tokens are always
/// literal text in FTS5, never operators.
///
/// Multiple tokens are joined with a space, which FTS5 treats as implicit
/// `AND`: a match must contain every token somewhere in the indexed text
/// (not necessarily adjacent or in order). Returns `None` for input with no
/// non-whitespace content, since `MATCH ''` is itself a syntax error.
pub(crate) fn escape_query(input: &str) -> Option<String> {
    let mut terms = input.split_whitespace().peekable();
    terms.peek()?;
    Some(
        terms
            .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_has_no_match_expression() {
        assert_eq!(escape_query(""), None);
        assert_eq!(escape_query("   "), None);
    }

    #[test]
    fn operators_and_wildcards_become_literal_tokens() {
        assert_eq!(escape_query("AND"), Some("\"AND\"".to_string()));
        assert_eq!(escape_query("*"), Some("\"*\"".to_string()));
        assert_eq!(escape_query("foo\"bar"), Some("\"foo\"\"bar\"".to_string()));
    }

    #[test]
    fn multiple_terms_join_as_implicit_and() {
        assert_eq!(
            escape_query("hello world"),
            Some("\"hello\" \"world\"".to_string())
        );
    }
}
