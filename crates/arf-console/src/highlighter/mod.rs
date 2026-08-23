//! Syntax highlighting for arf.
//!
//! This module provides syntax highlighting for both R code and meta commands.
//! R code is highlighted using tree-sitter-r for accurate parsing.
//!
pub mod bracket_match;
mod meta_command;
mod r_regex;
mod r_tree_sitter;

pub use meta_command::MetaCommandHighlighter;
pub use r_regex::TokenType;
pub use r_tree_sitter::{RTreeSitterHighlighter, tokenize_r};

use crate::config::ColorsConfig;
use crate::editor::mode::EditorStateRef;
use nu_ansi_term::Style;
use reedline::{AutoPairAction, AutoPairContext, Highlighter, StyledText};
use std::ops::Range;

const AUTO_PAIR_FOLLOWING_CLOSERS: [char; 6] = [')', ']', '}', '\'', '"', '`'];

/// Combined highlighter that handles both meta commands and R code.
///
/// Meta commands (lines starting with `:`) are highlighted in cyan.
/// R code is syntax-highlighted using tree-sitter-r.
pub struct CombinedHighlighter {
    meta_highlighter: MetaCommandHighlighter,
    r_highlighter: RTreeSitterHighlighter,
}

impl CombinedHighlighter {
    pub fn new(config: ColorsConfig, highlight_matching_bracket: bool) -> Self {
        CombinedHighlighter {
            meta_highlighter: MetaCommandHighlighter::new(config.meta),
            r_highlighter: RTreeSitterHighlighter::new(config.r, highlight_matching_bracket),
        }
    }

    /// Set the editor state reference used to resynchronize shadow state after
    /// history navigation and other out-of-band buffer changes.
    pub fn with_editor_state(mut self, state: EditorStateRef) -> Self {
        self.r_highlighter = self.r_highlighter.with_editor_state(state);
        self
    }
}

impl Default for CombinedHighlighter {
    fn default() -> Self {
        Self::new(ColorsConfig::default(), true)
    }
}

impl Highlighter for CombinedHighlighter {
    fn highlight(&self, line: &str, cursor: usize) -> StyledText {
        if line.trim_start().starts_with(':') {
            self.meta_highlighter.highlight(line, cursor)
        } else {
            self.r_highlighter.highlight(line, cursor)
        }
    }

    fn should_auto_pair(&self, context: &AutoPairContext<'_>) -> bool {
        if context.action() != AutoPairAction::Open {
            return true;
        }

        if context.selection().is_some() {
            return true;
        }

        should_open_auto_pair(
            context.buffer(),
            context.insertion_point(),
            context.pair(),
            context.selection(),
        )
    }
}

fn should_open_auto_pair(
    buffer: &str,
    insertion_point: usize,
    pair: (char, char),
    selection: Option<Range<usize>>,
) -> bool {
    if selection.is_some() {
        return true;
    }

    let positionally_allowed = insertion_point == buffer.len()
        || buffer[insertion_point..]
            .chars()
            .next()
            .is_some_and(|next| AUTO_PAIR_FOLLOWING_CLOSERS.contains(&next));
    if !positionally_allowed {
        return false;
    }

    let (open, close) = pair;
    close != open || !cursor_in_unclosed_delimiter(buffer, insertion_point, open)
}

/// Check whether `cursor` is inside an unclosed same-character delimiter.
///
/// Single quotes, double quotes, and backticks are tracked independently so a
/// delimiter appearing inside another delimiter's region is treated as text.
/// Backslash escapes are honored only while inside a delimiter. This helper is
/// intentionally lexical; R raw strings and comments require parser context
/// that is not available from this hook alone.
fn cursor_in_unclosed_delimiter(buffer: &str, cursor: usize, delimiter: char) -> bool {
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut escaped = false;

    for ch in buffer[..cursor].chars() {
        if escaped {
            escaped = false;
            continue;
        }

        if in_single {
            match ch {
                '\\' => escaped = true,
                '\'' => in_single = false,
                _ => {}
            }
            continue;
        }
        if in_double {
            match ch {
                '\\' => escaped = true,
                '"' => in_double = false,
                _ => {}
            }
            continue;
        }
        if in_backtick {
            match ch {
                '\\' => escaped = true,
                '`' => in_backtick = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '`' => in_backtick = true,
            _ => {}
        }
    }

    match delimiter {
        '\'' => in_single,
        '"' => in_double,
        '`' => in_backtick,
        _ => false,
    }
}

/// Simple highlighter that does no syntax highlighting.
///
/// Used when syntax highlighting is disabled.
#[allow(dead_code)]
pub struct NoHighlighter;

#[allow(dead_code)]
impl NoHighlighter {
    pub fn new() -> Self {
        NoHighlighter
    }
}

impl Default for NoHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl Highlighter for NoHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();
        styled.push((Style::new(), line.to_string()));
        styled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_combined_highlighter_meta_command() {
        let highlighter = CombinedHighlighter::default();
        let styled = highlighter.highlight(":help", 0);
        assert_eq!(styled.raw_string(), ":help");
        // Should be highlighted as meta command (single styled segment)
        assert_eq!(styled.buffer.len(), 1);
    }

    #[test]
    fn test_combined_highlighter_r_code() {
        let highlighter = CombinedHighlighter::default();
        let styled = highlighter.highlight("x <- 42", 0);
        assert_eq!(styled.raw_string(), "x <- 42");
        // Should have multiple styled segments (identifier, whitespace, operator, whitespace, number)
        assert!(styled.buffer.len() > 1);
    }

    #[test]
    fn test_combined_highlighter_meta_with_whitespace() {
        let highlighter = CombinedHighlighter::default();
        let styled = highlighter.highlight("  :reprex", 0);
        assert_eq!(styled.raw_string(), "  :reprex");
    }

    #[test]
    fn test_no_highlighter() {
        let highlighter = NoHighlighter::new();
        let styled = highlighter.highlight("x <- 42", 0);
        assert_eq!(styled.raw_string(), "x <- 42");
        assert_eq!(styled.buffer.len(), 1);
    }

    #[test]
    fn delimiter_context_tracks_each_delimiter_independently() {
        assert!(cursor_in_unclosed_delimiter("'text", 5, '\''));
        assert!(!cursor_in_unclosed_delimiter("'text\"", 6, '"'));
        assert!(cursor_in_unclosed_delimiter("\"text", 5, '"'));
        assert!(!cursor_in_unclosed_delimiter("\"text'", 6, '\''));
        assert!(cursor_in_unclosed_delimiter("`text", 5, '`'));
        assert!(!cursor_in_unclosed_delimiter("`text'", 6, '\''));
    }

    #[test]
    fn delimiter_context_honors_escapes_and_byte_cursor() {
        let escaped_double = r#"\"text\""#;
        assert!(cursor_in_unclosed_delimiter(
            escaped_double,
            escaped_double.len(),
            '"'
        ));

        let multibyte = "\"日";
        assert!(cursor_in_unclosed_delimiter(
            multibyte,
            multibyte.len(),
            '"'
        ));
        assert_eq!(multibyte.len(), 4, "cursor contract is a byte offset");
    }

    #[test]
    fn auto_pair_policy_allows_selection_wrap() {
        assert!(should_open_auto_pair("name", 2, ('(', ')'), Some(1..4)));
    }

    #[test]
    fn auto_pair_policy_requires_end_or_closer_without_selection() {
        assert!(!should_open_auto_pair("name", 2, ('(', ')'), None));
        assert!(should_open_auto_pair("name)", 4, ('(', ')'), None));
        assert!(should_open_auto_pair("name", 4, ('(', ')'), None));
    }

    #[test]
    fn auto_pair_policy_only_vetoes_same_delimiter_context() {
        assert!(!should_open_auto_pair("'text", 5, ('\'', '\''), None));
        assert!(should_open_auto_pair("'text", 5, ('"', '"'), None));
        assert!(should_open_auto_pair("'text", 5, ('(', ')'), None));
    }
}
