//! Pager content for the schema viewer.

use super::lines::extract_r_code_block;
use super::style::{StyleState, style_line_to_ratatui};
use crate::pager::lines_content::{CopyAction, LineStyler, LinesContent};
use ratatui::text::Line;
use std::cell::Cell;

/// Styles schema lines, tracking which code block the next line belongs to.
pub(super) struct SchemaStyler {
    /// Current style state for rendering (interior mutability for `style_line`).
    style_state: Cell<StyleState>,
}

impl SchemaStyler {
    fn new() -> Self {
        Self {
            style_state: Cell::new(StyleState::new()),
        }
    }
}

impl LineStyler for SchemaStyler {
    type Line = String;

    fn style_line(&self, lines: &[String], index: usize) -> Line<'static> {
        let line = &lines[index];
        let mut state = self.style_state.get();
        let styled = style_line_to_ratatui(line, &state);
        // Update state for the next line
        state.update(line);
        self.style_state.set(state);
        styled
    }

    fn prepare_render(&mut self, lines: &[String], scroll_offset: usize) {
        // Build state by scanning from the beginning up to scroll_offset
        let mut state = StyleState::new();
        for line in lines.iter().take(scroll_offset) {
            state.update(line);
        }
        self.style_state.set(state);
    }
}

/// Pager content for displaying the schema; `c` copies the R example.
pub(super) type SchemaContent = LinesContent<SchemaStyler>;

/// Build schema pager content from raw (unformatted) schema lines.
pub(super) fn schema_content(lines: Vec<String>) -> SchemaContent {
    let copy = CopyAction {
        text: extract_r_code_block(&lines),
        copied_label: "Copied R example to clipboard",
    };
    LinesContent::new(lines, SchemaStyler::new(), Some(copy))
}
