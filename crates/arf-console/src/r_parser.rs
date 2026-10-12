//! Shared tree-sitter utilities for parsing R code.
//!
//! This module provides common tree-sitter functionality used by:
//! - Syntax highlighting (`highlighter/r_tree_sitter.rs`)
//! - Hint acceptance (`editor/hinter.rs`)
//! - Word navigation (`editor/word_nav.rs`)
//! - Expression completeness checks (`editor/validator.rs`)

use std::cell::RefCell;
use std::ops::ControlFlow;
use tree_sitter::{Node, Parser, Tree};

// Thread-local tree-sitter parser for R, shared across modules.
thread_local! {
    static R_PARSER: RefCell<Parser> = RefCell::new({
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_r::LANGUAGE.into())
            .expect("Failed to set tree-sitter-r language");
        parser
    });
}

/// Parse R source code using a shared thread-local parser.
///
/// This function provides efficient parsing by reusing the same parser
/// instance within each thread. The parser is automatically initialized
/// on first use.
///
/// # Arguments
/// * `source` - The R source code to parse
///
/// # Returns
/// * `Some(Tree)` - The parsed syntax tree
/// * `None` - If parsing fails
///
/// # Example
/// ```ignore
/// if let Some(tree) = parse_r("x <- 1 + 2") {
///     let root = tree.root_node();
///     // ... work with the syntax tree
/// }
/// ```
pub fn parse_r(source: &str) -> Option<Tree> {
    R_PARSER.with(|parser| parser.borrow_mut().parse(source.as_bytes(), None))
}

/// Check if a tree-sitter node kind represents an atomic token.
///
/// Atomic tokens are nodes that should be treated as a single unit,
/// even if they have internal structure (e.g., strings with content and quotes).
///
/// This is used by:
/// - The highlighter to color entire strings/comments as one unit
/// - The hinter for token-based hint acceptance
/// - Word navigation for jumping over complete tokens
///
/// # Atomic node types
/// - Literals: `string`, `integer`, `float`, `complex`
/// - Comments: `comment`
/// - Constants: `true`, `false`, `null`, `inf`, `nan`, `na`
/// - Special: `dots` (`...`), `dot_dot_i` (`..1`, `..2`), `special` (`%foo%` operators)
pub fn is_atomic_node(kind: &str) -> bool {
    matches!(
        kind,
        "string"
            | "comment"
            | "integer"
            | "float"
            | "complex"
            | "true"
            | "false"
            | "null"
            | "inf"
            | "nan"
            | "na"
            | "dots"
            | "dot_dot_i"
            | "special" // %foo% operators
    )
}

/// Visit the atomic tokens of a syntax tree in document order.
///
/// A visited node is either an atomic node (see [`is_atomic_node`]), whose
/// children are not entered, or a leaf node with no children. Every other
/// node is descended into. The visitor receives nodes unfiltered, so it
/// can see empty nodes such as MISSING ones; callers decide which to keep.
/// Returning [`ControlFlow::Break`] stops the walk early.
pub fn walk_atomic_tokens<'tree>(
    root: Node<'tree>,
    mut visit: impl FnMut(Node<'tree>) -> ControlFlow<()>,
) {
    // A cursor created from a node never leaves that node's subtree.
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        if is_atomic_node(node.kind()) || node.child_count() == 0 {
            if visit(node).is_break() {
                return;
            }
        } else if cursor.goto_first_child() {
            continue;
        }

        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

/// Byte range of a visited token if navigation and hinting should treat it as
/// meaningful: non-empty, and not whitespace-only unless atomic.
fn meaningful_token_range(node: Node<'_>, source: &[u8]) -> Option<(usize, usize)> {
    let start = node.start_byte();
    let end = node.end_byte();
    if start >= end {
        return None;
    }
    if is_atomic_node(node.kind()) {
        return Some((start, end));
    }
    // Leaf node: skip whitespace-only tokens
    let text = std::str::from_utf8(&source[start..end]).ok()?;
    if text.chars().all(char::is_whitespace) {
        return None;
    }
    Some((start, end))
}

/// Collect the byte ranges of all meaningful atomic tokens, sorted by start.
pub fn collect_atomic_tokens(root: Node<'_>, source: &[u8]) -> Vec<(usize, usize)> {
    let mut tokens = Vec::new();
    walk_atomic_tokens(root, |node| {
        tokens.extend(meaningful_token_range(node, source));
        ControlFlow::Continue(())
    });
    tokens.sort_by_key(|(start, _)| *start);
    tokens
}

/// Byte range of the first meaningful atomic token in document order.
pub fn first_atomic_token(root: Node<'_>, source: &[u8]) -> Option<(usize, usize)> {
    let mut first = None;
    walk_atomic_tokens(root, |node| {
        first = meaningful_token_range(node, source);
        if first.is_some() {
            ControlFlow::Break(())
        } else {
            ControlFlow::Continue(())
        }
    });
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_r_simple() {
        let tree = parse_r("x <- 1");
        assert!(tree.is_some());
        let tree = tree.unwrap();
        assert_eq!(tree.root_node().kind(), "program");
    }

    #[test]
    fn test_parse_r_empty() {
        let tree = parse_r("");
        assert!(tree.is_some());
    }

    #[test]
    fn test_parse_r_complex() {
        let tree = parse_r("function(x) { x |> filter(y > 0) %>% select(-z) }");
        assert!(tree.is_some());
    }

    #[test]
    fn test_is_atomic_node() {
        for kind in [
            "string",
            "comment",
            "integer",
            "float",
            "complex",
            "true",
            "false",
            "null",
            "na",
            "special",
            "dots",
            "dot_dot_i",
        ] {
            assert!(is_atomic_node(kind), "{kind} should be atomic");
        }
        for kind in ["identifier", "program", "call", "binary_operator"] {
            assert!(!is_atomic_node(kind), "{kind} should not be atomic");
        }
    }

    fn tokens_of(source: &str) -> Vec<&str> {
        let tree = parse_r(source).unwrap();
        collect_atomic_tokens(tree.root_node(), source.as_bytes())
            .into_iter()
            .map(|(start, end)| &source[start..end])
            .collect()
    }

    #[test]
    fn test_collect_atomic_tokens_keeps_atomic_nodes_whole() {
        assert_eq!(
            tokens_of(r#"x |> f("a b", 1L) # note"#),
            ["x", "|>", "f", "(", r#""a b""#, ",", "1L", ")", "# note"]
        );
        assert_eq!(tokens_of("a %in% b"), ["a", "%in%", "b"]);
        assert_eq!(tokens_of(""), Vec::<&str>::new());
    }

    #[test]
    fn test_collect_atomic_tokens_skips_empty_missing_nodes() {
        // `f(` parses with a MISSING closing parenthesis that has no text.
        assert_eq!(tokens_of("f("), ["f", "("]);
    }

    #[test]
    fn test_first_atomic_token_and_early_exit() {
        let source = "  x <- 1";
        let tree = parse_r(source).unwrap();
        let first = first_atomic_token(tree.root_node(), source.as_bytes());
        assert_eq!(first, Some((2, 3)));

        let mut visited = 0;
        walk_atomic_tokens(tree.root_node(), |_| {
            visited += 1;
            ControlFlow::Break(())
        });
        assert_eq!(visited, 1);

        let empty = parse_r("").unwrap();
        assert_eq!(first_atomic_token(empty.root_node(), b""), None);
    }
}
