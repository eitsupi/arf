//! Shared pager content backed by a list of lines.
//!
//! [`LinesContent`] owns the lines, the feedback message and the optional
//! `c` copy action. How a line is turned into a styled [`Line`] (and any
//! state that depends on the scroll position or terminal width) is delegated
//! to a [`LineStyler`].

use super::{PagerAction, PagerContent, copy_to_clipboard};
use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::text::Line;

/// Turns stored lines into styled lines for rendering.
pub(super) trait LineStyler {
    /// Stored line type.
    type Line;

    /// Style the line at `index`.
    fn style_line(&self, lines: &[Self::Line], index: usize) -> Line<'static>;

    /// Called before rendering with the first visible line index.
    fn prepare_render(&mut self, _lines: &[Self::Line], _scroll_offset: usize) {}

    /// Called when the terminal width or height changes.
    fn on_resize(&mut self, _lines: &mut Vec<Self::Line>, _width: usize, _height: usize) {}
}

/// Text copied to the clipboard when the user presses `c`.
pub(super) struct CopyAction {
    /// Payload written to the clipboard.
    pub text: String,
    /// Feedback shown after a successful copy.
    pub copied_label: &'static str,
}

/// Pager content that shows a list of lines with an optional copy action.
pub(super) struct LinesContent<S: LineStyler> {
    lines: Vec<S::Line>,
    styler: S,
    copy: Option<CopyAction>,
    feedback_message: Option<String>,
}

impl<S: LineStyler> LinesContent<S> {
    pub fn new(lines: Vec<S::Line>, styler: S, copy: Option<CopyAction>) -> Self {
        Self {
            lines,
            styler,
            copy,
            feedback_message: None,
        }
    }

    /// Text that the `c` key copies, if any.
    #[cfg(test)]
    pub fn copy_text(&self) -> Option<&str> {
        self.copy.as_ref().map(|copy| copy.text.as_str())
    }
}

impl<S: LineStyler> PagerContent for LinesContent<S> {
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn render_line(&self, index: usize, _width: usize) -> Line<'static> {
        self.styler.style_line(&self.lines, index)
    }

    fn prepare_render(&mut self, scroll_offset: usize) {
        self.styler.prepare_render(&self.lines, scroll_offset);
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<PagerAction> {
        if code == KeyCode::Char('c')
            && modifiers == KeyModifiers::NONE
            && let Some(copy) = &self.copy
        {
            self.feedback_message = Some(if copy_to_clipboard(&copy.text).is_ok() {
                copy.copied_label.to_string()
            } else {
                "Failed to copy".to_string()
            });
        }
        // Never exit on copy; the feedback message is shown instead.
        None
    }

    fn feedback_message(&self) -> Option<&str> {
        self.feedback_message.as_deref()
    }

    fn clear_feedback(&mut self) {
        self.feedback_message = None;
    }

    fn on_resize(&mut self, width: usize, height: usize) {
        self.styler.on_resize(&mut self.lines, width, height);
    }
}
