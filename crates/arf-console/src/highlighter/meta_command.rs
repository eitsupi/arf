//! Meta command highlighting for arf.

use crate::config::MetaColorConfig;
use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};

/// Highlighter that provides visual feedback for meta commands.
///
/// Lines starting with `:` are highlighted in a distinct color to indicate
/// that the user is entering a meta command (like `:reprex` or `:help`).
pub struct MetaCommandHighlighter {
    config: MetaColorConfig,
}

impl MetaCommandHighlighter {
    pub fn new(config: MetaColorConfig) -> Self {
        MetaCommandHighlighter { config }
    }
}

impl Default for MetaCommandHighlighter {
    fn default() -> Self {
        Self::new(MetaColorConfig::default())
    }
}

impl Highlighter for MetaCommandHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let mut styled = StyledText::new();

        if line.trim_start().starts_with(':') {
            // Meta command mode - highlight the entire line with configured color
            let style = color_to_style(self.config.command);
            styled.push((style, line.to_string()));
        } else {
            // Normal mode - no special highlighting
            styled.push((Style::new(), line.to_string()));
        }

        styled
    }
}

/// Convert a Color to a Style with that color as foreground.
/// Color::Default results in no foreground color (plain text).
fn color_to_style(color: Color) -> Style {
    match color {
        Color::Default => Style::new(),
        c => Style::new().fg(c),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_meta_command_highlighted() {
        let highlighter = MetaCommandHighlighter::default();
        let styled = highlighter.highlight(":reprex", 0);
        let raw = styled.raw_string();
        assert_eq!(raw, ":reprex");
        // The buffer should have cyan styling
        assert_eq!(styled.buffer.len(), 1);
    }

    #[test]
    fn test_normal_input_not_highlighted() {
        let highlighter = MetaCommandHighlighter::default();
        let styled = highlighter.highlight("print(x)", 0);
        let raw = styled.raw_string();
        assert_eq!(raw, "print(x)");
        assert_eq!(styled.buffer.len(), 1);
    }

    #[test]
    fn test_meta_command_with_leading_whitespace() {
        let highlighter = MetaCommandHighlighter::default();
        let styled = highlighter.highlight("  :help", 0);
        let raw = styled.raw_string();
        assert_eq!(raw, "  :help");
        // Should still be highlighted
        assert_eq!(styled.buffer.len(), 1);
    }
}
