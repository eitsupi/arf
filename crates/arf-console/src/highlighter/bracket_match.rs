//! Bracket matching for R code.
//!
//! Finds the matching bracket for a given cursor position, using tree-sitter
//! to skip brackets inside strings and comments.
//!
//! This module is shared between:
//! - Bracket highlighting (visual feedback)
//! - Future bracket jump navigation

use tree_sitter::Tree;

/// Maximum number of bytes to scan when searching for a matching bracket.
const MAX_SCAN_DISTANCE: usize = 5000;

/// Bracket pairs supported for matching.
const BRACKET_PAIRS: [(u8, u8); 3] = [(b'(', b')'), (b'[', b']'), (b'{', b'}')];

/// Result of a bracket match: byte positions of both brackets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BracketMatch {
    /// Byte position of the first bracket (the one at/near cursor).
    pub cursor_bracket: usize,
    /// Byte position of the matching bracket.
    pub matching_bracket: usize,
}

/// Find the matching bracket at the given cursor position.
///
/// Trigger conditions (same as prompt_toolkit):
/// 1. Cursor is ON a bracket character (opening or closing)
/// 2. Cursor is immediately AFTER a closing bracket
///
/// Uses tree-sitter to skip brackets inside strings and comments.
pub fn find_matching_bracket(buffer: &str, cursor: usize, tree: &Tree) -> Option<BracketMatch> {
    let bytes = buffer.as_bytes();

    // Case 1: cursor is ON a bracket
    if cursor < bytes.len() && let Some(m) = try_match_at(bytes, cursor, tree) {
        return Some(m);
    }

    // Case 2: cursor is immediately AFTER a closing bracket
    if cursor > 0
        && cursor <= bytes.len()
        && is_closing_bracket(bytes[cursor - 1])
        && let Some(m) = try_match_at(bytes, cursor - 1, tree)
    {
        return Some(m);
    }

    None
}

/// Try to find a matching bracket at the given byte position.
fn try_match_at(bytes: &[u8], pos: usize, tree: &Tree) -> Option<BracketMatch> {
    let ch = bytes[pos];
    let (opening, closing, is_opening) = find_pair_for(ch)?;

    // Skip brackets inside strings or comments
    if is_in_string_or_comment(tree, pos) {
        return None;
    }

    // Search for the matching bracket
    let match_pos = if is_opening {
        scan_forward(bytes, pos, opening, closing, tree)?
    } else {
        scan_backward(bytes, pos, opening, closing, tree)?
    };

    Some(BracketMatch {
        cursor_bracket: pos,
        matching_bracket: match_pos,
    })
}

/// Find the bracket pair info for a character.
/// Returns (opening, closing, is_opening).
fn find_pair_for(ch: u8) -> Option<(u8, u8, bool)> {
    for &(open, close) in &BRACKET_PAIRS {
        if ch == open {
            return Some((open, close, true));
        }
        if ch == close {
            return Some((open, close, false));
        }
    }
    None
}

fn is_closing_bracket(ch: u8) -> bool {
    matches!(ch, b')' | b']' | b'}')
}

/// Scan forward from an opening bracket to find its closing match.
fn scan_forward(
    bytes: &[u8],
    start: usize,
    opening: u8,
    closing: u8,
    tree: &Tree,
) -> Option<usize> {
    let mut stack: i32 = 1;
    let limit = bytes.len().min(start + MAX_SCAN_DISTANCE);

    for (i, &ch) in bytes.iter().enumerate().take(limit).skip(start + 1) {
        if ch != opening && ch != closing {
            continue;
        }
        if is_in_string_or_comment(tree, i) {
            continue;
        }
        if ch == opening {
            stack += 1;
        } else {
            stack -= 1;
            if stack == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Scan backward from a closing bracket to find its opening match.
fn scan_backward(
    bytes: &[u8],
    start: usize,
    opening: u8,
    closing: u8,
    tree: &Tree,
) -> Option<usize> {
    let mut stack: i32 = 1;
    let limit = start.saturating_sub(MAX_SCAN_DISTANCE);

    for i in (limit..start).rev() {
        let ch = bytes[i];
        if ch != opening && ch != closing {
            continue;
        }
        if is_in_string_or_comment(tree, i) {
            continue;
        }
        if ch == closing {
            stack += 1;
        } else {
            stack -= 1;
            if stack == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Check if the given byte position is inside a string or comment node.
fn is_in_string_or_comment(tree: &Tree, byte_pos: usize) -> bool {
    let root = tree.root_node();
    let point = tree_sitter::Point {
        row: 0,
        column: byte_pos,
    };
    let node = root.descendant_for_point_range(point, point);

    match node {
        Some(n) => is_string_or_comment_kind(n.kind()),
        None => false,
    }
}

/// Check if a tree-sitter node kind represents a string or comment.
fn is_string_or_comment_kind(kind: &str) -> bool {
    matches!(
        kind,
        "string" | "string_content" | "escape_sequence" | "comment"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::r_parser::parse_r;

    fn match_at(input: &str, cursor: usize) -> Option<BracketMatch> {
        let tree = parse_r(input)?;
        find_matching_bracket(input, cursor, &tree)
    }

    #[test]
    fn test_simple_parens() {
        // "f(x)"
        //  0123
        let m = match_at("f(x)", 1).unwrap();
        assert_eq!(m.cursor_bracket, 1);
        assert_eq!(m.matching_bracket, 3);
    }

    #[test]
    fn test_simple_parens_from_close() {
        let m = match_at("f(x)", 3).unwrap();
        assert_eq!(m.cursor_bracket, 3);
        assert_eq!(m.matching_bracket, 1);
    }

    #[test]
    fn test_cursor_after_closing_paren() {
        // Cursor at position 4 (after ')'), should still match
        let m = match_at("f(x)", 4).unwrap();
        assert_eq!(m.cursor_bracket, 3);
        assert_eq!(m.matching_bracket, 1);
    }

    #[test]
    fn test_nested_parens() {
        // "f(g(x))"
        //  0123456
        let m = match_at("f(g(x))", 1).unwrap();
        assert_eq!(m.cursor_bracket, 1);
        assert_eq!(m.matching_bracket, 6);

        let m = match_at("f(g(x))", 3).unwrap();
        assert_eq!(m.cursor_bracket, 3);
        assert_eq!(m.matching_bracket, 5);
    }

    #[test]
    fn test_brackets() {
        // "x[1]"
        //  0123
        let m = match_at("x[1]", 1).unwrap();
        assert_eq!(m.cursor_bracket, 1);
        assert_eq!(m.matching_bracket, 3);
    }

    #[test]
    fn test_braces() {
        // "{ x }"
        //  01234
        let m = match_at("{ x }", 0).unwrap();
        assert_eq!(m.cursor_bracket, 0);
        assert_eq!(m.matching_bracket, 4);
    }

    #[test]
    fn test_unmatched_bracket() {
        assert!(match_at("f(x", 1).is_none());
    }

    #[test]
    fn test_bracket_in_string_skipped() {
        // The '(' inside the string should be skipped
        // 'paste("(", x)'
        //  0123456789012
        let input = r#"paste("(", x)"#;
        let m = match_at(input, 5).unwrap();
        assert_eq!(m.cursor_bracket, 5);
        assert_eq!(m.matching_bracket, 12);
    }

    #[test]
    fn test_bracket_in_comment_skipped() {
        // Brackets in comments should not be matched
        assert!(match_at("# f(x)", 3).is_none());
    }

    #[test]
    fn test_cursor_on_non_bracket() {
        assert!(match_at("hello", 2).is_none());
    }

    #[test]
    fn test_empty_input() {
        assert!(match_at("", 0).is_none());
    }

    #[test]
    fn test_mixed_bracket_types() {
        // "f(x[1])"
        //  0123456
        let m = match_at("f(x[1])", 1).unwrap();
        assert_eq!(m.cursor_bracket, 1);
        assert_eq!(m.matching_bracket, 6);

        let m = match_at("f(x[1])", 3).unwrap();
        assert_eq!(m.cursor_bracket, 3);
        assert_eq!(m.matching_bracket, 5);
    }

    #[test]
    fn test_cursor_after_closing_bracket() {
        // Cursor immediately after ']' should trigger
        let m = match_at("x[1]", 4).unwrap();
        assert_eq!(m.cursor_bracket, 3);
        assert_eq!(m.matching_bracket, 1);
    }

    #[test]
    fn test_cursor_after_closing_brace() {
        let m = match_at("{ x }", 5).unwrap();
        assert_eq!(m.cursor_bracket, 4);
        assert_eq!(m.matching_bracket, 0);
    }
}
