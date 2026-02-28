//! String context detection and path-in-string completion.
//!
//! Uses a thread-local tree-sitter R parser to determine whether the cursor
//! is inside a string literal, and provides Rust-native path completion for
//! paths typed inside strings.

use super::path::{PathCompletionOptions, complete_path};
use reedline::{Span, Suggestion};
use std::cell::RefCell;
use tree_sitter::{Parser, Tree};

// Thread-local tree-sitter parser for R.
thread_local! {
    static R_PARSER: RefCell<Parser> = RefCell::new({
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .expect("Failed to set tree-sitter-r language");
        parser
    });
}

/// Result of string context detection.
#[derive(Debug, Clone, PartialEq)]
pub struct StringContext {
    /// The partial path/content being typed inside the string.
    pub content: String,
    /// Start position of the string content (after opening quote).
    pub start: usize,
    /// The quote character used ('"' or '\'').
    pub quote: char,
}

/// Parse R code using tree-sitter.
fn parse_r_code(code: &str) -> Option<Tree> {
    R_PARSER.with(|parser| parser.borrow_mut().parse(code.as_bytes(), None))
}

/// Find the deepest node at or before the given byte position.
fn find_node_at_position<'a>(tree: &'a Tree, pos: usize) -> Option<tree_sitter::Node<'a>> {
    let root = tree.root_node();
    let mut cursor = root.walk();
    let mut best_node = None;

    // Walk down to find the deepest node containing the position
    loop {
        let node = cursor.node();

        // Check if position is within or at the end of this node
        if pos >= node.start_byte() && pos <= node.end_byte() {
            best_node = Some(node);

            // Try to go deeper
            if cursor.goto_first_child() {
                // Find the child that contains the position
                loop {
                    let child = cursor.node();
                    if pos >= child.start_byte() && pos <= child.end_byte() {
                        break; // Found the child, will process it in next iteration
                    }
                    if !cursor.goto_next_sibling() {
                        // No more siblings, go back to parent
                        cursor.goto_parent();
                        return best_node;
                    }
                }
            } else {
                // No children, this is the deepest node
                return best_node;
            }
        } else {
            return best_node;
        }
    }
}

/// Check if a node is a string node or inside a string.
fn find_string_ancestor<'a>(node: tree_sitter::Node<'a>) -> Option<tree_sitter::Node<'a>> {
    let mut current = Some(node);
    while let Some(n) = current {
        if n.kind() == "string" {
            return Some(n);
        }
        current = n.parent();
    }
    None
}

/// Check if a node is an ERROR node that contains an incomplete string.
/// Returns the position of the opening quote if found.
fn find_incomplete_string_in_error<'a>(
    node: tree_sitter::Node<'a>,
    source: &str,
) -> Option<(usize, char)> {
    let mut current = Some(node);
    while let Some(n) = current {
        if n.kind() == "ERROR" {
            // Look for quote characters in the ERROR node's text
            let start = n.start_byte();
            let end = n.end_byte().min(source.len());
            let text = &source[start..end];

            // Find the last opening quote that doesn't have a matching close
            let mut in_double = false;
            let mut in_single = false;
            let mut last_double_pos = None;
            let mut last_single_pos = None;
            let mut skip_next = false;

            for (i, c) in text.char_indices() {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                match c {
                    '\\' => {
                        // Skip next character (escape sequence)
                        skip_next = true;
                    }
                    '"' if !in_single => {
                        if in_double {
                            in_double = false;
                            last_double_pos = None;
                        } else {
                            in_double = true;
                            last_double_pos = Some(start + i);
                        }
                    }
                    '\'' if !in_double => {
                        if in_single {
                            in_single = false;
                            last_single_pos = None;
                        } else {
                            in_single = true;
                            last_single_pos = Some(start + i);
                        }
                    }
                    _ => {}
                }
            }

            // Return the unclosed quote position
            if in_double && let Some(pos) = last_double_pos {
                return Some((pos, '"'));
            }
            if in_single && let Some(pos) = last_single_pos {
                return Some((pos, '\''));
            }
        }
        current = n.parent();
    }
    None
}

/// Detect if cursor is inside a string literal using tree-sitter.
///
/// This uses tree-sitter-r for accurate parsing, correctly handling:
/// - Regular strings: "hello" or 'hello'
/// - Raw strings: r"(hello)" or R"(hello)"
/// - Escape sequences
/// - Comments (not detected as strings)
/// - Incomplete strings (handled via ERROR node analysis)
///
/// Returns `Some(StringContext)` if inside a string, `None` otherwise.
pub fn detect_string_context(line: &str, cursor_pos: usize) -> Option<StringContext> {
    // Parse the line
    let tree = parse_r_code(line)?;

    // Find the node at cursor position
    let node = find_node_at_position(&tree, cursor_pos)?;

    // First, check if we're inside a complete string
    if let Some(string_node) = find_string_ancestor(node) {
        // Get the string boundaries
        let string_start = string_node.start_byte();
        let string_end = string_node.end_byte();

        // Make sure cursor is actually inside the string (not at the closing quote)
        if cursor_pos < string_start || cursor_pos > string_end {
            return None;
        }

        // Extract the string content from the source
        let string_text = &line[string_start..string_end.min(line.len())];

        // Determine quote type and extract content
        let (quote_char, content_start_offset) = if string_text.starts_with("r\"")
            || string_text.starts_with("R\"")
            || string_text.starts_with("r'")
            || string_text.starts_with("R'")
        {
            // Raw string: r"(...)" - find the opening delimiter
            let quote = if string_text.contains('"') { '"' } else { '\'' };
            // Find position after r"( or similar
            let delim_end = string_text.find('(').map(|p| p + 1).unwrap_or(2);
            (quote, delim_end)
        } else if string_text.starts_with('"') {
            ('"', 1)
        } else if string_text.starts_with('\'') {
            ('\'', 1)
        } else {
            // Unknown string format
            return None;
        };

        // Calculate the absolute position where content starts
        let content_start = string_start + content_start_offset;

        // If cursor is before the content starts (in the opening quote/delimiter)
        if cursor_pos < content_start {
            return Some(StringContext {
                content: String::new(),
                start: content_start,
                quote: quote_char,
            });
        }

        // Extract content from content_start to cursor
        let content = if cursor_pos <= line.len() && content_start <= cursor_pos {
            line[content_start..cursor_pos].to_string()
        } else {
            String::new()
        };

        return Some(StringContext {
            content,
            start: content_start,
            quote: quote_char,
        });
    }

    // Check if we're in an ERROR node with an incomplete string
    if let Some((quote_pos, quote_char)) = find_incomplete_string_in_error(node, line) {
        // The content starts after the quote
        let content_start = quote_pos + 1;

        // Make sure cursor is after the quote
        if cursor_pos <= quote_pos {
            return None;
        }

        // Extract content from content_start to cursor
        let content = if cursor_pos <= line.len() && content_start <= cursor_pos {
            line[content_start..cursor_pos].to_string()
        } else {
            String::new()
        };

        return Some(StringContext {
            content,
            start: content_start,
            quote: quote_char,
        });
    }

    None
}

/// Convert path completions to reedline Suggestions.
pub(super) fn path_to_suggestions(
    partial: &str,
    pos: usize,
    span_start: usize,
    options: &PathCompletionOptions,
) -> Vec<Suggestion> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    complete_path(partial, &cwd, options)
        .into_iter()
        .map(|c| Suggestion {
            value: c.path,
            display_override: None,
            description: if c.is_dir {
                Some("directory".to_string())
            } else {
                None
            },
            extra: None,
            span: Span {
                start: span_start,
                end: pos,
            },
            append_whitespace: false,
            style: None,
            match_indices: c.match_indices,
        })
        .collect()
}

/// Complete paths using Rust-native path completion.
pub fn complete_path_in_string(_line: &str, pos: usize, ctx: &StringContext) -> Vec<Suggestion> {
    path_to_suggestions(
        &ctx.content,
        pos,
        ctx.start,
        &PathCompletionOptions::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- String context detection tests ---

    #[test]
    fn test_detect_string_context_double_quote() {
        // Inside double-quoted string
        let ctx = detect_string_context(r#"read.csv("data/"#, 15);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "data/");
        assert_eq!(ctx.quote, '"');
    }

    #[test]
    fn test_detect_string_context_single_quote() {
        // Inside single-quoted string
        let ctx = detect_string_context("source('script", 14);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "script");
        assert_eq!(ctx.quote, '\'');
    }

    #[test]
    fn test_detect_string_context_not_in_string() {
        // Not inside a string
        let ctx = detect_string_context("print(x)", 7);
        assert!(ctx.is_none());

        // After closing quote
        let ctx = detect_string_context(r#"read.csv("data.csv")"#, 20);
        assert!(ctx.is_none());
    }

    #[test]
    fn test_detect_string_context_with_escaped_quotes() {
        // Escaped quote should not close the string
        let ctx = detect_string_context(r#"paste("hello \"world"#, 20);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, r#"hello \"world"#);
    }

    #[test]
    fn test_detect_string_context_empty_string() {
        // Empty string (just opened quote)
        let ctx = detect_string_context(r#"read.csv(""#, 10);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "");
        assert_eq!(ctx.start, 10);
    }

    #[test]
    fn test_detect_string_context_tilde_path() {
        // Tilde path
        let ctx = detect_string_context(r#"setwd("~/"#, 9);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "~/");
    }

    #[test]
    fn test_detect_string_context_absolute_path() {
        // Absolute path
        let ctx = detect_string_context(r#"source("/usr/local/lib/"#, 23);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "/usr/local/lib/");
    }

    #[test]
    fn test_detect_string_context_complete_string_cursor_inside() {
        // Cursor inside a complete string
        let ctx = detect_string_context(r#"read.csv("data.csv")"#, 14);
        assert!(ctx.is_some());
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "data");
    }

    #[test]
    fn test_detect_string_context_in_comment() {
        // Quotes inside comments should NOT be detected as strings
        // This is one of the key benefits of using tree-sitter
        let ctx = detect_string_context(r#"# "data/"#, 8);
        assert!(ctx.is_none(), "Should not detect string inside comment");
    }

    #[test]
    fn test_detect_string_context_raw_string() {
        // R 4.0+ raw strings: r"(content)"
        // Inside a complete raw string at position 11 (after "hel")
        // x <- r"(hello)"
        // 0    5 78901234
        let ctx = detect_string_context(r#"x <- r"(hello)""#, 11);
        assert!(ctx.is_some(), "Should detect raw string");
        let ctx = ctx.unwrap();
        assert_eq!(ctx.content, "hel");
        assert_eq!(ctx.start, 8); // Content starts after r"(
    }
}
