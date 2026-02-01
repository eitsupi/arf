//! Custom prompt implementation.

use crate::config::prompt::ViSymbol;
use crate::config::{ModeIndicatorPosition, ViColorConfig};
use nu_ansi_term::{Color, Style};
use reedline::{
    Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus, PromptViMode,
};
use std::borrow::Cow;
use std::cell::Cell;

/// Simplified edit mode for internal tracking (implements Copy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum EditModeKind {
    #[default]
    ViInsert,
    ViNormal,
    NonVi,
}

impl From<PromptEditMode> for EditModeKind {
    fn from(mode: PromptEditMode) -> Self {
        match mode {
            PromptEditMode::Vi(PromptViMode::Insert) => EditModeKind::ViInsert,
            PromptEditMode::Vi(PromptViMode::Normal) => EditModeKind::ViNormal,
            _ => EditModeKind::NonVi,
        }
    }
}

/// Custom prompt for arf.
pub struct RPrompt {
    /// Mode indicator text (e.g., "[reprex] ").
    mode_indicator: Option<String>,
    /// Position of the mode indicator.
    mode_indicator_position: ModeIndicatorPosition,
    /// Main prompt string (may contain `{vi}` placeholder).
    prompt: String,
    /// Continuation prompt for multiline input (e.g., "+  ").
    continuation: String,
    /// Color for the main prompt.
    prompt_color: Color,
    /// Color for the continuation prompt.
    continuation_color: Color,
    /// Color for the mode indicator.
    mode_indicator_color: Color,
    /// Vi mode symbols for the prompt indicator.
    vi_symbol: ViSymbol,
    /// Vi mode colors for the prompt indicator.
    vi_colors: ViColorConfig,
    /// Whether the prompt contains `{vi}` placeholder.
    has_vi_placeholder: bool,
    /// Current edit mode (updated by render_prompt_indicator).
    current_edit_mode: Cell<EditModeKind>,
}

impl RPrompt {
    pub fn new(prompt: String, continuation: String) -> Self {
        let has_vi_placeholder = prompt.contains("{vi}");
        Self {
            mode_indicator: None,
            mode_indicator_position: ModeIndicatorPosition::Prefix,
            prompt,
            continuation,
            prompt_color: Color::Default,
            continuation_color: Color::Default,
            mode_indicator_color: Color::Default,
            vi_symbol: ViSymbol::default(),
            vi_colors: ViColorConfig::default(),
            has_vi_placeholder,
            current_edit_mode: Cell::new(EditModeKind::default()),
        }
    }

    pub fn with_mode_indicator(
        mut self,
        indicator: Option<String>,
        position: ModeIndicatorPosition,
    ) -> Self {
        self.mode_indicator = indicator;
        self.mode_indicator_position = position;
        self
    }

    pub fn with_colors(
        mut self,
        prompt: Color,
        continuation: Color,
        mode_indicator: Color,
    ) -> Self {
        self.prompt_color = prompt;
        self.continuation_color = continuation;
        self.mode_indicator_color = mode_indicator;
        self
    }

    pub fn with_vi_symbol(mut self, vi_symbol: ViSymbol) -> Self {
        self.vi_symbol = vi_symbol;
        self
    }

    pub fn with_vi_colors(mut self, vi_colors: ViColorConfig) -> Self {
        self.vi_colors = vi_colors;
        self
    }

    /// Get the vi symbol and color based on the current edit mode.
    fn get_vi_symbol_and_color(&self) -> (&str, Color) {
        match self.current_edit_mode.get() {
            EditModeKind::ViInsert => (&self.vi_symbol.insert, self.vi_colors.insert),
            EditModeKind::ViNormal => (&self.vi_symbol.normal, self.vi_colors.normal),
            EditModeKind::NonVi => (&self.vi_symbol.non_vi, self.vi_colors.non_vi),
        }
    }
}

impl Clone for RPrompt {
    fn clone(&self) -> Self {
        Self {
            mode_indicator: self.mode_indicator.clone(),
            mode_indicator_position: self.mode_indicator_position,
            prompt: self.prompt.clone(),
            continuation: self.continuation.clone(),
            prompt_color: self.prompt_color,
            continuation_color: self.continuation_color,
            mode_indicator_color: self.mode_indicator_color,
            vi_symbol: self.vi_symbol.clone(),
            vi_colors: self.vi_colors.clone(),
            has_vi_placeholder: self.has_vi_placeholder,
            current_edit_mode: self.current_edit_mode.clone(),
        }
    }
}

/// Convert a Color to a Style with that color as foreground.
fn color_to_style(color: Color) -> Style {
    match color {
        Color::Default => Style::new(),
        c => Style::new().fg(c),
    }
}

impl Prompt for RPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        let prompt_style = color_to_style(self.prompt_color);
        let indicator_style = color_to_style(self.mode_indicator_color);

        // Expand {vi} placeholder if present
        let prompt_text = if self.has_vi_placeholder {
            let (symbol, color) = self.get_vi_symbol_and_color();
            let vi_text = if symbol.is_empty() {
                String::new()
            } else {
                let style = color_to_style(color);
                style.paint(symbol).to_string()
            };
            self.prompt.replace("{vi}", &vi_text)
        } else {
            self.prompt.clone()
        };

        match (&self.mode_indicator, self.mode_indicator_position) {
            (Some(indicator), ModeIndicatorPosition::Prefix) => {
                let colored_indicator = indicator_style.paint(indicator).to_string();
                let colored_prompt = prompt_style.paint(&prompt_text).to_string();
                Cow::Owned(format!("{}{}", colored_indicator, colored_prompt))
            }
            _ => Cow::Owned(prompt_style.paint(&prompt_text).to_string()),
        }
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        let indicator_style = color_to_style(self.mode_indicator_color);

        match (&self.mode_indicator, self.mode_indicator_position) {
            (Some(indicator), ModeIndicatorPosition::Suffix) => {
                Cow::Owned(indicator_style.paint(indicator).to_string())
            }
            _ => Cow::Borrowed(""),
        }
    }

    fn render_prompt_indicator(&self, edit_mode: PromptEditMode) -> Cow<'_, str> {
        // Update stored mode for use by render_prompt_left
        self.current_edit_mode.set(edit_mode.into());

        // If the prompt has {vi} placeholder, we render the indicator there instead
        if self.has_vi_placeholder {
            return Cow::Borrowed("");
        }

        let (symbol, color) = self.get_vi_symbol_and_color();

        if symbol.is_empty() {
            Cow::Borrowed("")
        } else {
            let style = color_to_style(color);
            Cow::Owned(style.paint(symbol).to_string())
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        let style = color_to_style(self.continuation_color);
        Cow::Owned(style.paint(&self.continuation).to_string())
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };
        Cow::Owned(format!(
            "({}reverse-search: {}) ",
            prefix, history_search.term
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rprompt_without_mode_indicator() {
        let prompt = RPrompt::new("r> ".to_string(), "+  ".to_string());
        assert_eq!(prompt.render_prompt_left(), "r> ");
        assert_eq!(prompt.render_prompt_right(), "");
        assert_eq!(prompt.render_prompt_multiline_indicator(), "+  ");
    }

    #[test]
    fn test_rprompt_with_mode_indicator_prefix() {
        let prompt = RPrompt::new("r> ".to_string(), "+  ".to_string())
            .with_mode_indicator(Some("[reprex] ".to_string()), ModeIndicatorPosition::Prefix);
        assert_eq!(prompt.render_prompt_left(), "[reprex] r> ");
        assert_eq!(prompt.render_prompt_right(), "");
        assert_eq!(prompt.render_prompt_multiline_indicator(), "+  ");
    }

    #[test]
    fn test_rprompt_with_mode_indicator_suffix() {
        let prompt = RPrompt::new("r> ".to_string(), "+  ".to_string())
            .with_mode_indicator(Some("[reprex]".to_string()), ModeIndicatorPosition::Suffix);
        assert_eq!(prompt.render_prompt_left(), "r> ");
        assert_eq!(prompt.render_prompt_right(), "[reprex]");
        assert_eq!(prompt.render_prompt_multiline_indicator(), "+  ");
    }

    #[test]
    fn test_rprompt_with_mode_indicator_none() {
        let prompt = RPrompt::new("r> ".to_string(), "+  ".to_string())
            .with_mode_indicator(Some("[reprex] ".to_string()), ModeIndicatorPosition::None);
        // Even with indicator text, position=None should hide it
        assert_eq!(prompt.render_prompt_left(), "r> ");
        assert_eq!(prompt.render_prompt_right(), "");
    }

    #[test]
    fn test_rprompt_with_none_indicator_text() {
        let prompt = RPrompt::new("r> ".to_string(), "+  ".to_string())
            .with_mode_indicator(None, ModeIndicatorPosition::Prefix);
        assert_eq!(prompt.render_prompt_left(), "r> ");
        assert_eq!(prompt.render_prompt_right(), "");
    }

    #[test]
    fn test_rprompt_vi_insert_mode_indicator() {
        let vi_symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };
        let prompt = RPrompt::new("r> ".to_string(), "+  ".to_string()).with_vi_symbol(vi_symbol);

        let indicator = prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Insert));
        assert_eq!(indicator, "[I] ");
    }

    #[test]
    fn test_rprompt_vi_normal_mode_indicator() {
        let vi_symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };
        let prompt = RPrompt::new("r> ".to_string(), "+  ".to_string()).with_vi_symbol(vi_symbol);

        let indicator = prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Normal));
        assert_eq!(indicator, "[N] ");
    }

    #[test]
    fn test_rprompt_emacs_mode_indicator() {
        let vi_symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };
        let prompt = RPrompt::new("r> ".to_string(), "+  ".to_string()).with_vi_symbol(vi_symbol);

        let indicator = prompt.render_prompt_indicator(PromptEditMode::Emacs);
        assert_eq!(indicator, "[E] ");
    }

    #[test]
    fn test_rprompt_default_mode_indicator() {
        let vi_symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[D] ".to_string(),
        };
        let prompt = RPrompt::new("r> ".to_string(), "+  ".to_string()).with_vi_symbol(vi_symbol);

        // Default mode should use non_vi symbol
        let indicator = prompt.render_prompt_indicator(PromptEditMode::Default);
        assert_eq!(indicator, "[D] ");
    }

    #[test]
    fn test_rprompt_empty_vi_symbols() {
        // Default empty symbols
        let prompt = RPrompt::new("r> ".to_string(), "+  ".to_string());

        // All modes should return empty string with default (empty) symbols
        assert_eq!(
            prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Insert)),
            ""
        );
        assert_eq!(
            prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Normal)),
            ""
        );
        assert_eq!(prompt.render_prompt_indicator(PromptEditMode::Emacs), "");
    }

    #[test]
    fn test_rprompt_vi_placeholder_insert_mode() {
        let vi_symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };
        let prompt =
            RPrompt::new("{vi}r> ".to_string(), "+  ".to_string()).with_vi_symbol(vi_symbol);

        // First call render_prompt_indicator to set the mode
        let indicator = prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Insert));
        // With {vi} placeholder, indicator should be empty
        assert_eq!(indicator, "");

        // The prompt should contain the vi symbol
        let left = prompt.render_prompt_left();
        assert_eq!(left, "[I] r> ");
    }

    #[test]
    fn test_rprompt_vi_placeholder_normal_mode() {
        let vi_symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };
        let prompt =
            RPrompt::new("{vi}r> ".to_string(), "+  ".to_string()).with_vi_symbol(vi_symbol);

        // Set the mode to normal
        let indicator = prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Normal));
        assert_eq!(indicator, "");

        // The prompt should contain the normal mode symbol
        let left = prompt.render_prompt_left();
        assert_eq!(left, "[N] r> ");
    }

    #[test]
    fn test_rprompt_vi_placeholder_emacs_mode() {
        let vi_symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };
        let prompt =
            RPrompt::new("{vi}r> ".to_string(), "+  ".to_string()).with_vi_symbol(vi_symbol);

        // Set the mode to emacs (non-vi)
        let indicator = prompt.render_prompt_indicator(PromptEditMode::Emacs);
        assert_eq!(indicator, "");

        // The prompt should contain the non-vi symbol
        let left = prompt.render_prompt_left();
        assert_eq!(left, "[E] r> ");
    }

    #[test]
    fn test_rprompt_vi_placeholder_in_middle() {
        let vi_symbol = ViSymbol {
            insert: "I".to_string(),
            normal: "N".to_string(),
            non_vi: "E".to_string(),
        };
        let prompt =
            RPrompt::new("R [{vi}] > ".to_string(), "+  ".to_string()).with_vi_symbol(vi_symbol);

        prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Insert));
        assert_eq!(prompt.render_prompt_left(), "R [I] > ");

        prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Normal));
        assert_eq!(prompt.render_prompt_left(), "R [N] > ");
    }

    #[test]
    fn test_rprompt_vi_placeholder_with_empty_symbol() {
        // When vi symbols are empty, {vi} should expand to nothing
        let prompt = RPrompt::new("{vi}r> ".to_string(), "+  ".to_string());

        prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Insert));
        assert_eq!(prompt.render_prompt_left(), "r> ");
    }

    #[test]
    fn test_rprompt_without_vi_placeholder_uses_indicator() {
        // When there's no {vi} placeholder, render_prompt_indicator should return the symbol
        let vi_symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };
        let prompt = RPrompt::new("r> ".to_string(), "+  ".to_string()).with_vi_symbol(vi_symbol);

        let indicator = prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Insert));
        assert_eq!(indicator, "[I] ");

        // The prompt should NOT contain the vi symbol (no placeholder)
        assert_eq!(prompt.render_prompt_left(), "r> ");
    }

    #[test]
    fn test_rprompt_vi_placeholder_mode_switch() {
        // Test switching between modes
        let vi_symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };
        let prompt =
            RPrompt::new("{vi}r> ".to_string(), "+  ".to_string()).with_vi_symbol(vi_symbol);

        // Start in insert mode
        prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Insert));
        assert_eq!(prompt.render_prompt_left(), "[I] r> ");

        // Switch to normal mode
        prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Normal));
        assert_eq!(prompt.render_prompt_left(), "[N] r> ");

        // Switch back to insert mode
        prompt.render_prompt_indicator(PromptEditMode::Vi(PromptViMode::Insert));
        assert_eq!(prompt.render_prompt_left(), "[I] r> ");
    }
}
