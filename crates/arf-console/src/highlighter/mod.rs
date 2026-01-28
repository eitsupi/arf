//! Syntax highlighting for arf.
//!
//! This module provides syntax highlighting for both R code and meta commands.
//! R code is highlighted using tree-sitter-r for accurate parsing.
//!
//! The highlighter also synchronizes editor shadow state on every redraw,
//! keeping the state accurate even after history navigation.

mod meta_command;
mod r_regex;
mod r_tree_sitter;

pub use meta_command::MetaCommandHighlighter;
pub use r_tree_sitter::RTreeSitterHighlighter;

use crate::config::ColorsConfig;
use crate::editor::mode::EditorStateRef;
use nu_ansi_term::Style;
use reedline::{Highlighter, StyledText};

/// Combined highlighter that handles both meta commands and R code.
///
/// Meta commands (lines starting with `:`) are highlighted in cyan.
/// R code is syntax-highlighted using tree-sitter-r.
pub struct CombinedHighlighter {
    meta_highlighter: MetaCommandHighlighter,
    r_highlighter: RTreeSitterHighlighter,
}

impl CombinedHighlighter {
    pub fn new(config: ColorsConfig) -> Self {
        CombinedHighlighter {
            meta_highlighter: MetaCommandHighlighter::new(config.meta),
            r_highlighter: RTreeSitterHighlighter::new(config.r),
        }
    }

    /// Set the editor state reference for shadow state synchronization.
    ///
    /// When set, the R highlighter will sync the editor state with the actual
    /// buffer content and cursor position on every redraw. This ensures
    /// accurate state tracking even after history navigation.
    pub fn with_editor_state(mut self, state: EditorStateRef) -> Self {
        self.r_highlighter = self.r_highlighter.with_editor_state(state);
        self
    }
}

impl Default for CombinedHighlighter {
    fn default() -> Self {
        Self::new(ColorsConfig::default())
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
}
