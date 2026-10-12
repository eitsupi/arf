//! Token classification for R syntax highlighting.

use crate::config::RColorConfig;
use nu_ansi_term::{Color, Style};

/// Token types for R syntax highlighting.
/// Based on tree-sitter-r's highlights.scm definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenType {
    /// Comments starting with #
    Comment,
    /// String literals (single or double quoted, including raw strings)
    String,
    /// Numeric literals (integers, floats, hex, complex)
    Number,
    /// Reserved keywords (if, else, for, while, function, etc.)
    Keyword,
    /// Built-in constants (TRUE, FALSE, NULL, NA, Inf, NaN)
    Constant,
    /// Operators (+, -, <-, |>, %%, etc.)
    Operator,
    /// Punctuation (brackets, commas, semicolons)
    Punctuation,
    /// Identifiers (variable names, function names)
    Identifier,
    /// Whitespace
    Whitespace,
    /// Unrecognized text
    Other,
}

impl TokenType {
    /// Get the style for this token type from configuration.
    pub fn style(self, config: &RColorConfig) -> Style {
        match self {
            TokenType::Comment => color_to_style(config.comment),
            TokenType::String => color_to_style(config.string),
            TokenType::Number => color_to_style(config.number),
            TokenType::Keyword => color_to_style(config.keyword),
            TokenType::Constant => color_to_style(config.constant),
            TokenType::Operator => color_to_style(config.operator),
            TokenType::Punctuation => color_to_style(config.punctuation),
            TokenType::Identifier => color_to_style(config.identifier),
            TokenType::Whitespace => Style::new(),
            TokenType::Other => Style::new(),
        }
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
    fn test_style_uses_configured_color() {
        let config = RColorConfig {
            comment: Color::Green,
            string: Color::Default,
            ..RColorConfig::default()
        };
        assert_eq!(
            TokenType::Comment.style(&config),
            Style::new().fg(Color::Green)
        );
        assert_eq!(TokenType::String.style(&config), Style::new());
        assert_eq!(TokenType::Whitespace.style(&config), Style::new());
        assert_eq!(TokenType::Other.style(&config), Style::new());
    }
}
