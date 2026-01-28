//! Tree-sitter based word navigation for R code.
//!
//! This module provides R language-aware word navigation that uses tree-sitter
//! to properly identify token boundaries. Unlike unicode segmentation which
//! splits operators like `|>` into `|` and `>`, tree-sitter recognizes R
//! operators as single tokens.
//!
//! This enables Ctrl+Left/Right to jump over operators like `|>`, `<-`, `%>%`
//! as single units.

use crate::r_parser::{is_atomic_node, parse_r};

/// Find the position to move to when navigating left by one token.
///
/// Returns the byte position of the start of the previous token,
/// or 0 if at the beginning.
///
/// # Arguments
/// * `buffer` - The full buffer content
/// * `cursor_pos` - Current cursor position (byte offset)
pub fn token_left_position(buffer: &str, cursor_pos: usize) -> usize {
    if cursor_pos == 0 || buffer.is_empty() {
        return 0;
    }

    // Parse the buffer with tree-sitter (using shared parser)
    if let Some(tree) = parse_r(buffer) {
        let root = tree.root_node();

        // Find the token that contains or is just before the cursor
        if let Some(pos) = find_token_start_before(cursor_pos, &root, buffer.as_bytes()) {
            return pos;
        }
    }

    // Fallback: simple word-based navigation
    word_left_position_fallback(buffer, cursor_pos)
}

/// Find the position to move to when navigating right by one token.
///
/// Returns the byte position after the end of the next token,
/// or buffer.len() if at the end.
///
/// # Arguments
/// * `buffer` - The full buffer content
/// * `cursor_pos` - Current cursor position (byte offset)
pub fn token_right_position(buffer: &str, cursor_pos: usize) -> usize {
    if cursor_pos >= buffer.len() || buffer.is_empty() {
        return buffer.len();
    }

    // Parse the buffer with tree-sitter (using shared parser)
    if let Some(tree) = parse_r(buffer) {
        let root = tree.root_node();

        // Find the token that starts at or after the cursor
        if let Some(pos) = find_token_end_after(cursor_pos, &root, buffer.as_bytes()) {
            return pos;
        }
    }

    // Fallback: simple word-based navigation
    word_right_position_fallback(buffer, cursor_pos)
}

/// Find the start position of the token before (or containing) the cursor.
fn find_token_start_before(
    cursor_pos: usize,
    root: &tree_sitter::Node,
    source: &[u8],
) -> Option<usize> {
    // Collect all tokens with their positions
    let tokens = collect_tokens(root, source);

    // Skip any whitespace before cursor to find the previous token
    let effective_pos = skip_whitespace_left(source, cursor_pos);

    // Find the token that ends at or before effective_pos
    let mut best_token: Option<(usize, usize)> = None;

    for (start, end) in &tokens {
        if *end <= effective_pos {
            // Token ends before or at cursor - candidate for "previous token"
            best_token = Some((*start, *end));
        } else if *start < effective_pos && effective_pos < *end {
            // Cursor is inside this token - move to start of this token
            return Some(*start);
        }
    }

    // If we found a token before cursor, return its start
    if let Some((start, _)) = best_token {
        return Some(start);
    }

    None
}

/// Find the end position of the token at or after the cursor.
fn find_token_end_after(
    cursor_pos: usize,
    root: &tree_sitter::Node,
    source: &[u8],
) -> Option<usize> {
    // Collect all tokens with their positions
    let tokens = collect_tokens(root, source);

    // Skip any whitespace after cursor
    let effective_pos = skip_whitespace_right(source, cursor_pos);

    // Find the first token that starts at or after effective_pos
    for (start, end) in &tokens {
        if *start >= effective_pos {
            // Token starts at or after cursor - return its end
            return Some(*end);
        } else if *start < effective_pos && effective_pos < *end {
            // Cursor is inside this token - move to end of this token
            return Some(*end);
        }
    }

    None
}

/// Collect all meaningful tokens from the syntax tree.
fn collect_tokens(root: &tree_sitter::Node, source: &[u8]) -> Vec<(usize, usize)> {
    let mut tokens = Vec::new();
    collect_tokens_recursive(root, source, &mut tokens);
    tokens.sort_by_key(|(start, _)| *start);
    tokens
}

/// Recursively collect tokens from the syntax tree.
fn collect_tokens_recursive(
    node: &tree_sitter::Node,
    source: &[u8],
    tokens: &mut Vec<(usize, usize)>,
) {
    let kind = node.kind();

    // If this is an atomic node, treat it as a single token
    if is_atomic_node(kind) {
        let start = node.start_byte();
        let end = node.end_byte();
        if start < end {
            tokens.push((start, end));
        }
        return;
    }

    // If this is a leaf node (no children), it's a token
    if node.child_count() == 0 {
        let start = node.start_byte();
        let end = node.end_byte();
        if start < end {
            // Skip whitespace-only tokens
            if let Ok(text) = std::str::from_utf8(&source[start..end]) {
                if !text.chars().all(char::is_whitespace) {
                    tokens.push((start, end));
                }
            }
        }
        return;
    }

    // Recurse into children
    for i in 0..node.child_count() {
        if let Some(child) = node.child(i) {
            collect_tokens_recursive(&child, source, tokens);
        }
    }
}

/// Skip whitespace to the left of the given position.
fn skip_whitespace_left(source: &[u8], pos: usize) -> usize {
    let mut p = pos;
    while p > 0 {
        let prev_char_start = find_char_start(source, p - 1);
        if let Ok(s) = std::str::from_utf8(&source[prev_char_start..p]) {
            if let Some(c) = s.chars().next() {
                if c.is_whitespace() {
                    p = prev_char_start;
                    continue;
                }
            }
        }
        break;
    }
    p
}

/// Skip whitespace to the right of the given position.
fn skip_whitespace_right(source: &[u8], pos: usize) -> usize {
    let mut p = pos;
    while p < source.len() {
        if let Ok(s) = std::str::from_utf8(&source[p..]) {
            if let Some(c) = s.chars().next() {
                if c.is_whitespace() {
                    p += c.len_utf8();
                    continue;
                }
            }
        }
        break;
    }
    p
}

/// Find the start of a UTF-8 character containing the given byte position.
fn find_char_start(source: &[u8], pos: usize) -> usize {
    let mut p = pos;
    while p > 0 && (source[p] & 0xC0) == 0x80 {
        p -= 1;
    }
    p
}

/// Fallback: simple word-based navigation to the left.
fn word_left_position_fallback(buffer: &str, cursor_pos: usize) -> usize {
    let before = &buffer[..cursor_pos];

    // Skip trailing whitespace
    let trimmed = before.trim_end();
    if trimmed.is_empty() {
        return 0;
    }

    // Find the start of the last word
    if let Some(last_space) = trimmed.rfind(char::is_whitespace) {
        last_space + 1
    } else {
        0
    }
}

/// Fallback: simple word-based navigation to the right.
fn word_right_position_fallback(buffer: &str, cursor_pos: usize) -> usize {
    let after = &buffer[cursor_pos..];

    // Skip leading whitespace
    let trimmed = after.trim_start();
    if trimmed.is_empty() {
        return buffer.len();
    }

    let whitespace_len = after.len() - trimmed.len();

    // Find the end of the first word
    if let Some(first_space) = trimmed.find(char::is_whitespace) {
        cursor_pos + whitespace_len + first_space
    } else {
        buffer.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_right_pipe_operator() {
        // `|>` should be treated as a single token
        let buffer = "x |> filter()";
        // Cursor after "x " (position 2), should jump to end of "|>"
        let pos = token_right_position(buffer, 2);
        assert_eq!(pos, 4); // After "|>"
    }

    #[test]
    fn test_token_right_assignment() {
        // `<-` should be a single token
        let buffer = "x <- 42";
        // Cursor after "x " (position 2)
        let pos = token_right_position(buffer, 2);
        assert_eq!(pos, 4); // After "<-"
    }

    #[test]
    fn test_token_left_pipe_operator() {
        // Moving left from after "|>" should go to start of "|>"
        let buffer = "x |> filter()";
        // Cursor after "|>" (position 4)
        let pos = token_left_position(buffer, 4);
        assert_eq!(pos, 2); // Start of "|>"
    }

    #[test]
    fn test_token_left_assignment() {
        // Moving left from after "<-" should go to start of "<-"
        let buffer = "x <- 42";
        // Cursor after "<-" (position 4)
        let pos = token_left_position(buffer, 4);
        assert_eq!(pos, 2); // Start of "<-"
    }

    #[test]
    fn test_token_right_identifier() {
        let buffer = "filter(data)";
        // Cursor at start
        let pos = token_right_position(buffer, 0);
        assert_eq!(pos, 6); // After "filter"
    }

    #[test]
    fn test_token_left_identifier() {
        let buffer = "filter(data)";
        // Cursor after "filter"
        let pos = token_left_position(buffer, 6);
        assert_eq!(pos, 0); // Start of "filter"
    }

    #[test]
    fn test_token_right_at_end() {
        let buffer = "x <- 1";
        let pos = token_right_position(buffer, buffer.len());
        assert_eq!(pos, buffer.len());
    }

    #[test]
    fn test_token_left_at_start() {
        let buffer = "x <- 1";
        let pos = token_left_position(buffer, 0);
        assert_eq!(pos, 0);
    }

    #[test]
    fn test_token_right_double_arrow() {
        // `<<-` should be a single token
        let buffer = "x <<- 42";
        let pos = token_right_position(buffer, 2);
        assert_eq!(pos, 5); // After "<<-"
    }

    #[test]
    fn test_token_right_magrittr_pipe() {
        // `%>%` should be a single token
        let buffer = "x %>% y";
        let pos = token_right_position(buffer, 2);
        assert_eq!(pos, 5); // After "%>%"
    }

    #[test]
    fn test_token_right_comparison() {
        // `>=` should be a single token
        let buffer = "x >= 5";
        let pos = token_right_position(buffer, 2);
        assert_eq!(pos, 4); // After ">="
    }

    #[test]
    fn test_token_right_logical_and() {
        // `&&` should be a single token
        let buffer = "x && y";
        let pos = token_right_position(buffer, 2);
        assert_eq!(pos, 4); // After "&&"
    }

    #[test]
    fn test_token_right_namespace() {
        // `::` should be a single token
        let buffer = "dplyr::filter";
        let pos = token_right_position(buffer, 5);
        assert_eq!(pos, 7); // After "::"
    }

    #[test]
    fn test_token_right_string() {
        // String should be a single token
        let buffer = r#"x <- "hello world""#;
        let pos = token_right_position(buffer, 5);
        assert_eq!(pos, 18); // After the entire string
    }

    #[test]
    fn test_empty_buffer() {
        let buffer = "";
        assert_eq!(token_left_position(buffer, 0), 0);
        assert_eq!(token_right_position(buffer, 0), 0);
    }

    #[test]
    fn test_whitespace_only() {
        let buffer = "   ";
        assert_eq!(token_left_position(buffer, 3), 0);
        assert_eq!(token_right_position(buffer, 0), 3);
    }

    #[test]
    fn test_skip_whitespace_then_token() {
        let buffer = "  x <- 1";
        // From position 0, should skip whitespace and find "x"
        let pos = token_right_position(buffer, 0);
        assert_eq!(pos, 3); // After "x"
    }
}
