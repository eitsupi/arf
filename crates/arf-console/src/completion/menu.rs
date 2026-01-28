//! Custom menu wrappers with shadow state synchronization.
//!
//! This module provides wrappers for reedline menus that synchronize the shadow
//! editor state after buffer modifications. This is critical for features like
//! auto-match and bracket deletion that depend on accurate buffer tracking.
//!
//! ## FunctionAwareMenu
//! Wraps IdeMenu with:
//! - Cursor positioning for function completions (cursor inside parentheses)
//! - Shadow state sync after completion and partial completion
//!
//! ## StateSyncHistoryMenu
//! Wraps ListMenu with:
//! - Shadow state sync after history selection

use crate::editor::mode::EditorStateRef;
use reedline::{
    Completer, Editor, IdeMenu, ListMenu, Menu, MenuEvent, Painter, Suggestion, UndoBehavior,
};

/// Custom completion menu that adjusts cursor position for function completions.
pub struct FunctionAwareMenu {
    inner: IdeMenu,
    /// Shared editor state for shadow tracking synchronization.
    editor_state: Option<EditorStateRef>,
}

impl FunctionAwareMenu {
    /// Create a new FunctionAwareMenu wrapping an IdeMenu.
    pub fn new(inner: IdeMenu) -> Self {
        Self {
            inner,
            editor_state: None,
        }
    }

    /// Set the editor state reference for shadow tracking synchronization.
    pub fn with_editor_state(mut self, state: EditorStateRef) -> Self {
        self.editor_state = Some(state);
        self
    }

    /// Synchronize the shadow state with the actual editor buffer.
    ///
    /// This is critical after any operation that modifies the buffer in ways
    /// the shadow tracking system cannot predict (e.g., completion, partial
    /// completion). Without this sync, auto_match and bracket_delete rules
    /// will malfunction.
    fn sync_editor_state(&self, editor: &Editor) {
        if let Some(state_ref) = &self.editor_state {
            if let Ok(mut state) = state_ref.lock() {
                let buffer = editor.get_buffer();
                let cursor_pos = editor.line_buffer().insertion_point();

                // Update shadow state to match actual buffer
                state.buffer = buffer.to_string();
                state.buffer_len = buffer.chars().count();
                state.cursor_pos = buffer[..cursor_pos].chars().count();
                state.uncertain = false;
            }
        }
    }
}

impl Menu for FunctionAwareMenu {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn indicator(&self) -> &str {
        self.inner.indicator()
    }

    fn is_active(&self) -> bool {
        self.inner.is_active()
    }

    fn menu_event(&mut self, event: MenuEvent) {
        self.inner.menu_event(event);
    }

    fn can_quick_complete(&self) -> bool {
        self.inner.can_quick_complete()
    }

    fn can_partially_complete(
        &mut self,
        values_updated: bool,
        editor: &mut Editor,
        completer: &mut dyn Completer,
    ) -> bool {
        let result = self
            .inner
            .can_partially_complete(values_updated, editor, completer);

        // If partial completion occurred, synchronize the shadow state.
        // Partial completion inserts the common prefix among completions,
        // which modifies the buffer in ways the shadow tracking cannot predict.
        if result {
            self.sync_editor_state(editor);
        }

        result
    }

    fn update_values(&mut self, editor: &mut Editor, completer: &mut dyn Completer) {
        self.inner.update_values(editor, completer);
    }

    fn update_working_details(
        &mut self,
        editor: &mut Editor,
        completer: &mut dyn Completer,
        painter: &Painter,
    ) {
        self.inner
            .update_working_details(editor, completer, painter);
    }

    fn replace_in_buffer(&self, editor: &mut Editor) {
        // Get buffer state before replacement
        let before_len = editor.get_buffer().len();

        // Let the inner menu do the replacement
        self.inner.replace_in_buffer(editor);

        // Check if the text just before the cursor is "()" (function completion)
        // This works even when completing inside another function call like debug(print())
        let buffer = editor.get_buffer();
        let cursor_pos = editor.line_buffer().insertion_point();
        let is_function_completion = cursor_pos >= 2
            && buffer.len() > before_len
            && buffer.get(cursor_pos - 2..cursor_pos) == Some("()");

        if is_function_completion {
            editor.edit_buffer(
                |line_buffer| {
                    line_buffer.move_left();
                },
                UndoBehavior::MoveCursor,
            );
        }

        // Synchronize the shadow state with the actual buffer state.
        self.sync_editor_state(editor);
    }

    fn min_rows(&self) -> u16 {
        self.inner.min_rows()
    }

    fn get_values(&self) -> &[Suggestion] {
        self.inner.get_values()
    }

    fn menu_required_lines(&self, terminal_columns: u16) -> u16 {
        self.inner.menu_required_lines(terminal_columns)
    }

    fn menu_string(&self, available_lines: u16, use_ansi_coloring: bool) -> String {
        self.inner.menu_string(available_lines, use_ansi_coloring)
    }

    fn set_cursor_pos(&mut self, pos: (u16, u16)) {
        self.inner.set_cursor_pos(pos);
    }
}

/// History menu wrapper that synchronizes shadow state after selection.
///
/// This wraps reedline's ListMenu to sync the shadow editor state after
/// a history entry is selected. Without this sync, auto_match and bracket_delete
/// rules will malfunction after using history completion.
pub struct StateSyncHistoryMenu {
    inner: ListMenu,
    /// Shared editor state for shadow tracking synchronization.
    editor_state: Option<EditorStateRef>,
}

impl StateSyncHistoryMenu {
    /// Create a new StateSyncHistoryMenu wrapping a ListMenu.
    pub fn new(inner: ListMenu) -> Self {
        Self {
            inner,
            editor_state: None,
        }
    }

    /// Set the editor state reference for shadow tracking synchronization.
    pub fn with_editor_state(mut self, state: EditorStateRef) -> Self {
        self.editor_state = Some(state);
        self
    }

    /// Synchronize the shadow state with the actual editor buffer.
    fn sync_editor_state(&self, editor: &Editor) {
        if let Some(state_ref) = &self.editor_state {
            if let Ok(mut state) = state_ref.lock() {
                let buffer = editor.get_buffer();
                let cursor_pos = editor.line_buffer().insertion_point();

                // Update shadow state to match actual buffer
                state.buffer = buffer.to_string();
                state.buffer_len = buffer.chars().count();
                state.cursor_pos = buffer[..cursor_pos].chars().count();
                state.uncertain = false;
            }
        }
    }
}

impl Menu for StateSyncHistoryMenu {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn indicator(&self) -> &str {
        self.inner.indicator()
    }

    fn is_active(&self) -> bool {
        self.inner.is_active()
    }

    fn menu_event(&mut self, event: MenuEvent) {
        self.inner.menu_event(event);
    }

    fn can_quick_complete(&self) -> bool {
        self.inner.can_quick_complete()
    }

    fn can_partially_complete(
        &mut self,
        values_updated: bool,
        editor: &mut Editor,
        completer: &mut dyn Completer,
    ) -> bool {
        let result = self
            .inner
            .can_partially_complete(values_updated, editor, completer);

        if result {
            self.sync_editor_state(editor);
        }

        result
    }

    fn update_values(&mut self, editor: &mut Editor, completer: &mut dyn Completer) {
        self.inner.update_values(editor, completer);
    }

    fn update_working_details(
        &mut self,
        editor: &mut Editor,
        completer: &mut dyn Completer,
        painter: &Painter,
    ) {
        self.inner
            .update_working_details(editor, completer, painter);
    }

    fn replace_in_buffer(&self, editor: &mut Editor) {
        self.inner.replace_in_buffer(editor);

        // Synchronize the shadow state with the actual buffer state.
        self.sync_editor_state(editor);
    }

    fn min_rows(&self) -> u16 {
        self.inner.min_rows()
    }

    fn get_values(&self) -> &[Suggestion] {
        self.inner.get_values()
    }

    fn menu_required_lines(&self, terminal_columns: u16) -> u16 {
        self.inner.menu_required_lines(terminal_columns)
    }

    fn menu_string(&self, available_lines: u16, use_ansi_coloring: bool) -> String {
        self.inner.menu_string(available_lines, use_ansi_coloring)
    }

    fn set_cursor_pos(&mut self, pos: (u16, u16)) {
        self.inner.set_cursor_pos(pos);
    }
}
