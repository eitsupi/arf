//! Custom prompt implementation.

use crate::config::ModeIndicatorPosition;
use nu_ansi_term::{Color, Style};
use reedline::{Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus};
use std::borrow::Cow;

/// Custom prompt for arf.
pub struct RPrompt {
    /// Mode indicator text (e.g., "[reprex] ").
    mode_indicator: Option<String>,
    /// Position of the mode indicator.
    mode_indicator_position: ModeIndicatorPosition,
    /// Main prompt string (e.g., "r> ").
    prompt: String,
    /// Continuation prompt for multiline input (e.g., "+  ").
    continuation: String,
    /// Color for the main prompt.
    prompt_color: Color,
    /// Color for the continuation prompt.
    continuation_color: Color,
    /// Color for the mode indicator.
    mode_indicator_color: Color,
}

impl RPrompt {
    pub fn new(prompt: String, continuation: String) -> Self {
        Self {
            mode_indicator: None,
            mode_indicator_position: ModeIndicatorPosition::Prefix,
            prompt,
            continuation,
            prompt_color: Color::Default,
            continuation_color: Color::Default,
            mode_indicator_color: Color::Default,
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

        match (&self.mode_indicator, self.mode_indicator_position) {
            (Some(indicator), ModeIndicatorPosition::Prefix) => {
                let colored_indicator = indicator_style.paint(indicator).to_string();
                let colored_prompt = prompt_style.paint(&self.prompt).to_string();
                Cow::Owned(format!("{}{}", colored_indicator, colored_prompt))
            }
            _ => Cow::Owned(prompt_style.paint(&self.prompt).to_string()),
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

    fn render_prompt_indicator(&self, _edit_mode: PromptEditMode) -> Cow<'_, str> {
        // No indicator - the prompt string already includes everything
        Cow::Borrowed("")
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
}
