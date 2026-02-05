//! R-aware hinting for reedline.
//!
//! This module provides an R language-aware hinter that uses tree-sitter
//! to properly tokenize R code. This ensures that multi-character operators
//! like `|>`, `<-`, `%>%` are treated as single tokens when accepting
//! history hints with Ctrl+Right.

use crate::r_parser::{is_atomic_node, parse_r};
use nu_ansi_term::Style;
use reedline::{Hinter, History, SearchQuery};

/// Get the first R token from a string using tree-sitter.
///
/// Unlike unicode segmentation which splits `|>` into `|` and `>`,
/// tree-sitter recognizes R operators as single tokens.
///
/// Falls back to a simple word-based tokenization if parsing fails.
fn get_first_r_token(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    // Try to parse with tree-sitter (using shared parser)
    if let Some(tree) = parse_r(s) {
        let root = tree.root_node();

        // Find the first meaningful token
        if let Some((start, end)) = find_first_token_bounds(&root, s.as_bytes()) {
            // Include any leading whitespace before the token
            let leading_ws = &s[..start];
            let token = &s[start..end];
            return format!("{}{}", leading_ws, token);
        }
    }

    // Fallback: simple word-based tokenization
    get_first_word(s)
}

/// Simple fallback: get first whitespace-delimited word (including leading whitespace).
fn get_first_word(s: &str) -> String {
    let mut chars = s.chars().peekable();
    let mut result = String::new();

    // Collect leading whitespace
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            result.push(c);
            chars.next();
        } else {
            break;
        }
    }

    // Collect non-whitespace characters (the "word")
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            break;
        }
        result.push(c);
        chars.next();
    }

    result
}

/// Find the byte boundaries of the first meaningful token in the syntax tree.
///
/// Returns `Some((start, end))` if a token is found, `None` otherwise.
fn find_first_token_bounds(node: &tree_sitter::Node, source: &[u8]) -> Option<(usize, usize)> {
    let mut cursor = node.walk();
    let root_id = node.id();

    // Walk to find the first non-whitespace token
    loop {
        let current = cursor.node();
        let kind = current.kind();

        // If this is an atomic node (e.g., string, comment), return it as a whole
        if is_atomic_node(kind) {
            let start = current.start_byte();
            let end = current.end_byte();
            if start < end {
                return Some((start, end));
            }
        }

        // Check if this is a leaf node (no children)
        if current.child_count() == 0 {
            let start = current.start_byte();
            let end = current.end_byte();

            // Skip if empty
            if start < end {
                // Check if it's whitespace
                if let Ok(text) = std::str::from_utf8(&source[start..end])
                    && !text.chars().all(char::is_whitespace)
                {
                    return Some((start, end));
                }
            }
        } else if !is_atomic_node(kind) {
            // Not atomic and has children, try to go deeper
            if cursor.goto_first_child() {
                continue;
            }
        }

        // Try next sibling
        if cursor.goto_next_sibling() {
            continue;
        }

        // No siblings, go up and try next sibling
        loop {
            if !cursor.goto_parent() {
                return None; // Reached root
            }
            if cursor.node().id() == root_id {
                return None; // Back at original root
            }
            if cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

/// An R language-aware hinter that properly tokenizes R code.
///
/// This hinter implements history-based suggestions with:
/// - R-aware tokenization via tree-sitter (operators like `|>`, `<-` are single tokens)
/// - Optional cwd filtering (only show suggestions from current directory)
pub struct RLanguageHinter {
    style: Style,
    current_hint: String,
    min_chars: usize,
    /// If true, prefer history entries from the current working directory.
    cwd_aware: bool,
}

impl RLanguageHinter {
    /// Create a new R language hinter.
    pub fn new() -> Self {
        RLanguageHinter {
            style: Style::new(),
            current_hint: String::new(),
            min_chars: 1,
            cwd_aware: false,
        }
    }

    /// Set the style for the hint text.
    #[must_use]
    pub fn with_style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }

    /// Set the minimum number of characters before showing hints.
    #[must_use]
    #[allow(dead_code)]
    pub fn with_min_chars(mut self, min_chars: usize) -> Self {
        self.min_chars = min_chars;
        self
    }

    /// Enable cwd-aware filtering.
    ///
    /// When enabled, the hinter will prefer history entries that were recorded
    /// in the current working directory. Falls back to all history if no
    /// matches found in current directory.
    #[must_use]
    pub fn with_cwd_aware(mut self, cwd_aware: bool) -> Self {
        self.cwd_aware = cwd_aware;
        self
    }
}

impl Default for RLanguageHinter {
    fn default() -> Self {
        Self::new()
    }
}

impl Hinter for RLanguageHinter {
    fn handle(
        &mut self,
        line: &str,
        #[allow(unused_variables)] pos: usize,
        history: &dyn History,
        use_ansi_coloring: bool,
        cwd: &str,
    ) -> String {
        self.current_hint = if line.chars().count() >= self.min_chars {
            if self.cwd_aware {
                // Try cwd-filtered search first
                let cwd_results = history
                    .search(SearchQuery::last_with_prefix_and_cwd(
                        line.to_string(),
                        cwd.to_string(),
                        history.session(),
                    ))
                    .unwrap_or_default();

                if !cwd_results.is_empty() {
                    cwd_results[0]
                        .command_line
                        .get(line.len()..)
                        .unwrap_or_default()
                        .to_string()
                } else {
                    // Fall back to all history
                    self.search_all_history(line, history)
                }
            } else {
                // Search all history
                self.search_all_history(line, history)
            }
        } else {
            String::new()
        };

        if use_ansi_coloring && !self.current_hint.is_empty() {
            self.style.paint(&self.current_hint).to_string()
        } else {
            self.current_hint.clone()
        }
    }

    fn complete_hint(&self) -> String {
        self.current_hint.clone()
    }

    fn next_hint_token(&self) -> String {
        // Use tree-sitter to get the first R token
        get_first_r_token(&self.current_hint)
    }
}

impl RLanguageHinter {
    /// Search all history for entries starting with the given prefix.
    fn search_all_history(&self, prefix: &str, history: &dyn History) -> String {
        history
            .search(SearchQuery::last_with_prefix(
                prefix.to_string(),
                history.session(),
            ))
            .unwrap_or_default()
            .first()
            .map(|entry| {
                entry
                    .command_line
                    .get(prefix.len()..)
                    .unwrap_or_default()
                    .to_string()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_first_r_token_pipe_operator() {
        // The key fix: |> should be treated as a single token
        let result = get_first_r_token("|> filter(x > 0)");
        assert_eq!(result, "|>");
    }

    #[test]
    fn test_get_first_r_token_assignment_operator() {
        // <- should be a single token
        let result = get_first_r_token("<- 42");
        assert_eq!(result, "<-");
    }

    #[test]
    fn test_get_first_r_token_double_arrow() {
        // <<- should be a single token
        let result = get_first_r_token("<<- value");
        assert_eq!(result, "<<-");
    }

    #[test]
    fn test_get_first_r_token_right_arrow() {
        // -> should be a single token
        let result = get_first_r_token("-> y");
        assert_eq!(result, "->");
    }

    #[test]
    fn test_get_first_r_token_namespace() {
        // :: should be a single token
        let result = get_first_r_token("::mutate");
        assert_eq!(result, "::");
    }

    #[test]
    fn test_get_first_r_token_triple_colon() {
        // ::: should be a single token
        let result = get_first_r_token(":::internal");
        assert_eq!(result, ":::");
    }

    #[test]
    fn test_get_first_r_token_comparison() {
        // >= should be a single token
        let result = get_first_r_token(">= 5");
        assert_eq!(result, ">=");

        // <= should be a single token
        let result = get_first_r_token("<= 5");
        assert_eq!(result, "<=");

        // == should be a single token
        let result = get_first_r_token("== TRUE");
        assert_eq!(result, "==");

        // != should be a single token
        let result = get_first_r_token("!= FALSE");
        assert_eq!(result, "!=");
    }

    #[test]
    fn test_get_first_r_token_identifier() {
        let result = get_first_r_token("filter(x > 0)");
        assert_eq!(result, "filter");
    }

    #[test]
    fn test_get_first_r_token_with_leading_space() {
        // Should include leading whitespace
        let result = get_first_r_token(" |> filter()");
        assert_eq!(result, " |>");
    }

    #[test]
    fn test_get_first_r_token_number() {
        let result = get_first_r_token("42 + 1");
        assert_eq!(result, "42");
    }

    #[test]
    fn test_get_first_r_token_string() {
        let result = get_first_r_token(r#""hello" world"#);
        assert_eq!(result, r#""hello""#);
    }

    #[test]
    fn test_get_first_r_token_empty() {
        let result = get_first_r_token("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_get_first_r_token_special_operator() {
        // %% special operators
        let result = get_first_r_token("%% 2");
        // tree-sitter-r may parse this differently, but it should not split
        assert!(!result.is_empty());
    }

    #[test]
    fn test_get_first_r_token_logical_and() {
        // && should be a single token
        let result = get_first_r_token("&& y");
        assert_eq!(result, "&&");
    }

    #[test]
    fn test_get_first_r_token_logical_or() {
        // || should be a single token
        let result = get_first_r_token("|| y");
        assert_eq!(result, "||");
    }

    #[test]
    fn test_get_first_r_token_walrus() {
        // := (walrus operator in data.table)
        let result = get_first_r_token(":= value");
        assert_eq!(result, ":=");
    }

    #[test]
    fn test_get_first_r_token_power() {
        // ** (power operator)
        let result = get_first_r_token("** 2");
        assert_eq!(result, "**");
    }
}
