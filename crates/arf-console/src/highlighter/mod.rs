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
    if matches!((open, close), ('\'', '\'') | ('"', '"'))
        && is_r_raw_string_prefix(buffer, insertion_point)
    {
        return false;
    }
    close != open || !cursor_in_unclosed_delimiter(buffer, insertion_point, open)
}

#[derive(Default)]
struct DelimiterContext {
    in_single: bool,
    in_double: bool,
    in_backtick: bool,
    in_comment: bool,
    raw_string: Option<RawStringContext>,
}

#[derive(Clone, Copy)]
struct RawStringContext {
    close: char,
    quote: char,
    dashes: usize,
}

/// Check whether `cursor` is inside an unclosed same-character delimiter.
///
/// Single quotes, double quotes, and backticks are tracked independently so a
/// delimiter appearing inside another delimiter's region is treated as text.
/// Backslash escapes are honored only while inside a delimiter. This is a
/// lexical approximation, but it also tracks comments and R raw-string
/// delimiters so quotes in those regions cannot be mistaken for pair starts.
fn cursor_in_unclosed_delimiter(buffer: &str, cursor: usize, delimiter: char) -> bool {
    let context = delimiter_context(buffer, cursor);

    if let Some(raw) = context.raw_string
        && raw.quote == delimiter
    {
        return true;
    }

    match delimiter {
        '\'' => context.in_single,
        '"' => context.in_double,
        '`' => context.in_backtick,
        _ => false,
    }
}

fn delimiter_context(buffer: &str, cursor: usize) -> DelimiterContext {
    let mut in_single = false;
    let mut in_double = false;
    let mut in_backtick = false;
    let mut in_comment = false;
    let mut escaped = false;
    let mut raw_string: Option<RawStringContext> = None;
    let source = &buffer[..cursor];
    let mut index = 0;

    while index < source.len() {
        let ch = source[index..]
            .chars()
            .next()
            .expect("index is always on a character boundary");
        let ch_len = ch.len_utf8();

        if let Some(raw) = raw_string {
            if ch == raw.close && raw_string_ends_at(&source[index..], raw) {
                let suffix_len = raw.dashes + raw.quote.len_utf8();
                index += ch_len + suffix_len;
                raw_string = None;
                continue;
            }
            index += ch_len;
            continue;
        }

        if in_comment {
            if ch == '\n' {
                in_comment = false;
            }
            index += ch_len;
            continue;
        }

        if escaped {
            escaped = false;
            index += ch_len;
            continue;
        }

        if in_single {
            match ch {
                '\\' => escaped = true,
                '\'' => in_single = false,
                _ => {}
            }
            index += ch_len;
            continue;
        }
        if in_double {
            match ch {
                '\\' => escaped = true,
                '"' => in_double = false,
                _ => {}
            }
            index += ch_len;
            continue;
        }
        if in_backtick {
            match ch {
                '\\' => escaped = true,
                '`' => in_backtick = false,
                _ => {}
            }
            index += ch_len;
            continue;
        }

        if matches!(ch, 'r' | 'R')
            && raw_string_boundary(source, index)
            && let Some((raw, consumed)) = parse_raw_string_start(&source[index..])
        {
            // Skip the raw prefix and its opening delimiter. Quotes inside
            // the raw body are data, not ordinary R string delimiters.
            raw_string = Some(raw);
            index += consumed;
            continue;
        }

        match ch {
            '#' => in_comment = true,
            '\'' => in_single = true,
            '"' => in_double = true,
            '`' => in_backtick = true,
            _ => {}
        }
        index += ch_len;
    }

    DelimiterContext {
        in_single,
        in_double,
        in_backtick,
        in_comment,
        raw_string,
    }
}

fn raw_string_boundary(source: &str, index: usize) -> bool {
    source[..index]
        .chars()
        .next_back()
        .is_none_or(|previous| !previous.is_alphanumeric() && !matches!(previous, '_' | '.'))
}

fn parse_raw_string_start(source: &str) -> Option<(RawStringContext, usize)> {
    let bytes = source.as_bytes();
    if bytes.len() < 3 || !matches!(bytes[0], b'r' | b'R') {
        return None;
    }

    let quote = match bytes[1] {
        b'\'' => '\'',
        b'"' => '"',
        _ => return None,
    };
    let mut index = 2;
    while bytes.get(index) == Some(&b'-') {
        index += 1;
    }

    let (close, dashes) = match bytes.get(index) {
        Some(b'(') => (')', index - 2),
        Some(b'[') => (']', index - 2),
        Some(b'{') => ('}', index - 2),
        _ => return None,
    };

    Some((
        RawStringContext {
            close,
            quote,
            dashes,
        },
        index + 1,
    ))
}

fn raw_string_ends_at(source: &str, raw: RawStringContext) -> bool {
    let mut index = raw.close.len_utf8();
    for _ in 0..raw.dashes {
        if source.get(index..).and_then(|rest| rest.as_bytes().first()) != Some(&b'-') {
            return false;
        }
        index += 1;
    }
    source.get(index..).and_then(|rest| rest.chars().next()) == Some(raw.quote)
}

fn is_r_raw_string_prefix(buffer: &str, insertion_point: usize) -> bool {
    let prefix = &buffer[..insertion_point];
    let Some((r_position, raw_prefix)) = prefix.char_indices().next_back() else {
        return false;
    };
    if !matches!(raw_prefix, 'r' | 'R') {
        return false;
    }

    if let Some(previous) = prefix[..r_position].chars().next_back()
        && (previous.is_alphanumeric() || matches!(previous, '_' | '.'))
    {
        return false;
    }

    let context = delimiter_context(buffer, insertion_point);
    !context.in_single && !context.in_double && !context.in_backtick && !context.in_comment
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

    #[test]
    fn raw_string_prefix_suppresses_quote_pairing() {
        assert!(is_r_raw_string_prefix("r", 1));
        assert!(is_r_raw_string_prefix("R", 1));
        assert!(is_r_raw_string_prefix("x <- r", 6));
        assert!(is_r_raw_string_prefix("(r", 2));
        assert!(!is_r_raw_string_prefix("ar", 2));
        assert!(!is_r_raw_string_prefix("foo.r", 5));
        assert!(!is_r_raw_string_prefix("my_r", 4));

        assert!(!should_open_auto_pair("r", 1, ('"', '"'), None));
        assert!(!should_open_auto_pair("R", 1, ('\'', '\''), None));
        assert!(should_open_auto_pair("r", 1, ('`', '`'), None));
        assert!(should_open_auto_pair("r", 1, ('(', ')'), None));
    }

    #[test]
    fn raw_string_prefix_is_not_detected_inside_strings_or_comments() {
        assert!(!is_r_raw_string_prefix("\"r", 2));
        assert!(!is_r_raw_string_prefix("'r", 2));
        assert!(!is_r_raw_string_prefix("`r", 2));
        assert!(!is_r_raw_string_prefix("# r", 3));
        assert!(!is_r_raw_string_prefix("x # r", 5));

        // Once the raw string has started, the closing quote is vetoed by the
        // raw-string context rather than treated as a new pair.
        let raw_body = "r\"(text)";
        assert!(!should_open_auto_pair(
            raw_body,
            raw_body.len(),
            ('"', '"'),
            None
        ));
    }

    #[test]
    fn raw_string_context_allows_internal_quotes_and_dashed_delimiters() {
        for (body, quote) in [
            ("r\"(hello \"world\")", '"'),
            ("r'---(hello \"world\")---", '\''),
            ("r\"[hello \"world\"]", '"'),
            ("r\"{hello \"world\"}", '"'),
        ] {
            assert!(
                cursor_in_unclosed_delimiter(body, body.len(), quote),
                "raw body should keep its matching quote open: {body}"
            );
            assert!(!should_open_auto_pair(
                body,
                body.len(),
                (quote, quote),
                None
            ));
        }

        let complete = "r\"---(hello \"world\")---\"";
        assert!(!cursor_in_unclosed_delimiter(complete, complete.len(), '"'));
        assert!(should_open_auto_pair(
            complete,
            complete.len(),
            ('"', '"'),
            None
        ));
    }

    #[test]
    fn auto_pair_policy_allows_bracket_pairs_at_buffer_end() {
        for pair in [('(', ')'), ('[', ']'), ('{', '}')] {
            assert!(should_open_auto_pair("text", 4, pair, None));
        }
    }
}
