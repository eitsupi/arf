//! Syntax highlighting for arf.
//!
//! This module provides syntax highlighting for both R code and meta commands.
//! R code is highlighted using tree-sitter-r for accurate parsing.
//!
mod auto_pair;
pub mod bracket_match;
mod meta_command;
mod r_tree_sitter;
mod token;

pub use meta_command::MetaCommandHighlighter;
pub use r_tree_sitter::{RTreeSitterHighlighter, tokenize_r};
pub use token::TokenType;

use self::auto_pair::should_open_auto_pair;
use crate::config::ColorsConfig;
use crate::editor::mode::EditorStateRef;
use nu_ansi_term::Style;
use reedline::{AutoPairAction, AutoPairContext, Highlighter, StyledText};

/// Combined highlighter that handles both meta commands and R code.
///
/// Meta commands (lines starting with `:`) are highlighted in cyan.
/// R code is syntax-highlighted using tree-sitter-r.
pub struct CombinedHighlighter {
    meta_highlighter: MetaCommandHighlighter,
    r_highlighter: RTreeSitterHighlighter,
    editor_state: Option<EditorStateRef>,
}

impl CombinedHighlighter {
    pub fn new(config: ColorsConfig, highlight_matching_bracket: bool) -> Self {
        CombinedHighlighter {
            meta_highlighter: MetaCommandHighlighter::new(config.meta),
            r_highlighter: RTreeSitterHighlighter::new(config.r, highlight_matching_bracket),
            editor_state: None,
        }
    }

    /// Set the editor state reference used to resynchronize shadow state after
    /// history navigation and other out-of-band buffer changes.
    pub fn with_editor_state(mut self, state: EditorStateRef) -> Self {
        self.editor_state = Some(state);
        self
    }

    fn sync_editor_state(&self, line: &str, cursor: usize) {
        if let Some(state_ref) = &self.editor_state
            && let Ok(mut state) = state_ref.lock()
        {
            state.buffer = line.to_string();
            state.buffer_len = line.chars().count();
            state.cursor_pos = line
                .char_indices()
                .take_while(|(byte_pos, _)| *byte_pos < cursor)
                .count();
            state.uncertain = false;
        }
    }
}

impl Default for CombinedHighlighter {
    fn default() -> Self {
        Self::new(ColorsConfig::default(), true)
    }
}

impl Highlighter for CombinedHighlighter {
    fn highlight(&self, line: &str, cursor: usize) -> StyledText {
        self.sync_editor_state(line, cursor);
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
    fn combined_highlighter_syncs_editor_state_for_r_code_with_utf8_cursor() {
        let editor_state = crate::editor::mode::new_editor_state_ref();
        let highlighter = CombinedHighlighter::default().with_editor_state(editor_state.clone());
        let line = "x <- '日'";
        let cursor = line.len() - 1;
        {
            let mut state = editor_state.lock().unwrap();
            state.uncertain = true;
        }

        highlighter.highlight(line, cursor);

        let state = editor_state.lock().unwrap();
        assert_eq!(state.buffer, line);
        assert_eq!(state.buffer_len, 8);
        assert_eq!(state.cursor_pos, 7);
        assert!(!state.uncertain);
    }

    #[test]
    fn combined_highlighter_syncs_editor_state_for_indented_meta_commands() {
        let editor_state = crate::editor::mode::new_editor_state_ref();
        let highlighter = CombinedHighlighter::default().with_editor_state(editor_state.clone());
        let line = "  :reprex";
        {
            let mut state = editor_state.lock().unwrap();
            state.buffer = "stale".to_string();
            state.buffer_len = 5;
            state.cursor_pos = 5;
            state.uncertain = true;
        }

        highlighter.highlight(line, 4);

        let state = editor_state.lock().unwrap();
        assert_eq!(state.buffer, line);
        assert_eq!(state.buffer_len, line.chars().count());
        assert_eq!(state.cursor_pos, 4);
        assert!(!state.uncertain);
    }

    #[test]
    fn test_no_highlighter() {
        let highlighter = NoHighlighter::new();
        let styled = highlighter.highlight("x <- 42", 0);
        assert_eq!(styled.raw_string(), "x <- 42");
        assert_eq!(styled.buffer.len(), 1);
    }
}
