//! Shared tree-sitter utilities for parsing R code.
//!
//! This module provides common tree-sitter functionality used by:
//! - Syntax highlighting (`highlighter/r_tree_sitter.rs`)
//! - Hint acceptance (`editor/hinter.rs`)
//! - Word navigation (`editor/word_nav.rs`)

use std::cell::RefCell;
use tree_sitter::{Parser, Tree};

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
    fn test_is_atomic_node_string() {
        assert!(is_atomic_node("string"));
    }

    #[test]
    fn test_is_atomic_node_comment() {
        assert!(is_atomic_node("comment"));
    }

    #[test]
    fn test_is_atomic_node_numbers() {
        assert!(is_atomic_node("integer"));
        assert!(is_atomic_node("float"));
        assert!(is_atomic_node("complex"));
    }

    #[test]
    fn test_is_atomic_node_constants() {
        assert!(is_atomic_node("true"));
        assert!(is_atomic_node("false"));
        assert!(is_atomic_node("null"));
        assert!(is_atomic_node("na"));
    }

    #[test]
    fn test_is_atomic_node_special() {
        assert!(is_atomic_node("special"));
        assert!(is_atomic_node("dots"));
        assert!(is_atomic_node("dot_dot_i"));
    }

    #[test]
    fn test_is_atomic_node_non_atomic() {
        assert!(!is_atomic_node("identifier"));
        assert!(!is_atomic_node("program"));
        assert!(!is_atomic_node("call"));
        assert!(!is_atomic_node("binary_operator"));
    }
}
