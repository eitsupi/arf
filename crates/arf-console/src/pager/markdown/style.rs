//! Styles and syntax-highlight colors used by the Markdown writer.

use crate::config::RColorConfig;
use crate::highlighter::TokenType;
use crate::pager::style_convert::nu_ansi_color_to_ratatui;
use ratatui::style::{Color, Style};

pub(super) struct Styles {
    pub(super) h1: Style,
    pub(super) h2: Style,
    pub(super) h3: Style,
    pub(super) code: Style,
    pub(super) emphasis: Style,
    pub(super) strong: Style,
    pub(super) link: Style,
    pub(super) blockquote_prefix: Style,
    pub(super) code_block: Style,
    /// Background color applied to entire code block lines (via `Line::style`).
    pub(super) code_block_bg: Color,
}

impl Default for Styles {
    fn default() -> Self {
        Self {
            h1: Style::new().bold().underlined(),
            h2: Style::new().bold(),
            h3: Style::new().italic(),
            code: Style::new().cyan(),
            emphasis: Style::new().italic(),
            strong: Style::new().bold(),
            link: Style::new().cyan().underlined(),
            blockquote_prefix: Style::new().green(),
            code_block: Style::new().dim(),
            code_block_bg: Color::Indexed(236),
        }
    }
}

/// Map a `TokenType` to a `ratatui::style::Style` using default R colors.
///
/// Derives colors from [`RColorConfig::default()`] via [`nu_ansi_color_to_ratatui`]
/// so the pager stays in sync with the REPL color scheme.
pub(super) fn token_type_to_style(tt: TokenType) -> Style {
    // Use a static default so we only build it once.
    static CONFIG: std::sync::LazyLock<RColorConfig> =
        std::sync::LazyLock::new(RColorConfig::default);

    let color = match tt {
        TokenType::Comment => CONFIG.comment,
        TokenType::String => CONFIG.string,
        TokenType::Number => CONFIG.number,
        TokenType::Keyword => CONFIG.keyword,
        TokenType::Constant => CONFIG.constant,
        TokenType::Operator => CONFIG.operator,
        TokenType::Punctuation => CONFIG.punctuation,
        TokenType::Identifier => CONFIG.identifier,
        TokenType::Whitespace | TokenType::Other => return Style::default(),
    };

    match color {
        nu_ansi_term::Color::Default => Style::default(),
        c => Style::new().fg(nu_ansi_color_to_ratatui(c)),
    }
}

/// Check if a code block language tag indicates R code.
pub(super) fn is_r_language(lang: &str) -> bool {
    matches!(lang, "r" | "R")
}
