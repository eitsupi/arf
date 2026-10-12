//! Event loop and key handling for the history browser.

use super::render::MIN_SIZE;
use super::{HistoryBrowser, HistoryBrowserResult};
use crate::pager::{check_terminal_too_small, copy_to_clipboard, with_alternate_screen};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind,
};
use std::io;
use std::time::Duration;

impl HistoryBrowser {
    /// Run the browser and return the result.
    pub(super) fn run(&mut self) -> io::Result<HistoryBrowserResult> {
        with_alternate_screen(|| self.run_inner())
    }

    fn run_inner(&mut self) -> io::Result<HistoryBrowserResult> {
        let mut stdout = io::stdout();
        let poll_timeout = Duration::from_millis(50);
        let mut needs_redraw = true;
        let mut too_small;

        loop {
            // Update animation state
            if self.update_text_scroll() {
                needs_redraw = true;
            }

            too_small = check_terminal_too_small(&MIN_SIZE).is_some();
            if needs_redraw {
                self.render(&mut stdout)?;
                needs_redraw = false;
            }

            if event::poll(poll_timeout)? {
                let ev = event::read()?;
                log::trace!("history_browser: received event: {:?}", ev);
                match ev {
                    Event::Key(key) => {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        needs_redraw = true;

                        if let Some(result) = self.handle_key(key, too_small)? {
                            return Ok(result);
                        }
                    }
                    Event::Mouse(mouse) => {
                        if self.handle_mouse(mouse.kind) {
                            needs_redraw = true;
                        }
                    }
                    Event::Resize(_, _) => {
                        needs_redraw = true;
                    }
                    _ => {}
                }
            }
        }
    }

    /// Handle a key press. Returns `Some` when the browser should exit.
    fn handle_key(
        &mut self,
        key: KeyEvent,
        too_small: bool,
    ) -> io::Result<Option<HistoryBrowserResult>> {
        // When the terminal is too small, only accept exit keys
        // to prevent input from leaking into filter or other state.
        if too_small {
            return Ok(Self::handle_key_too_small(key));
        }

        self.feedback_message = None;

        // Handle delete confirmation dialog
        if self.show_delete_dialog {
            self.handle_key_dialog(key)?;
            return Ok(None);
        }

        if self.filter_active {
            Ok(self.handle_key_filter(key))
        } else {
            Ok(self.handle_key_normal(key))
        }
    }

    fn handle_key_too_small(key: KeyEvent) -> Option<HistoryBrowserResult> {
        match (key.code, key.modifiers) {
            (KeyCode::Esc, _)
            | (KeyCode::Char('q'), KeyModifiers::NONE)
            | (KeyCode::Char('c'), KeyModifiers::CONTROL)
            | (KeyCode::Char('d'), KeyModifiers::CONTROL) => Some(HistoryBrowserResult::Cancelled),
            _ => None,
        }
    }

    fn handle_key_dialog(&mut self, key: KeyEvent) -> io::Result<()> {
        match key.code {
            KeyCode::Enter => {
                self.show_delete_dialog = false;
                self.delete_selected()?;
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.show_delete_dialog = false;
                self.feedback_message = Some("Delete cancelled".to_string());
            }
            _ => {}
        }
        Ok(())
    }

    /// Filter mode: all char input goes to filter text.
    fn handle_key_filter(&mut self, key: KeyEvent) -> Option<HistoryBrowserResult> {
        match (key.code, key.modifiers) {
            // Confirm filter and return to normal mode
            (KeyCode::Enter, _) => {
                self.filter_active = false;
            }

            // Clear filter and return to normal mode
            (KeyCode::Esc, _) => {
                self.filter.raw_query.clear();
                self.filter.cursor_pos = 0;
                self.filter.reparse();
                self.update_filter();
                self.filter_active = false;
            }

            // Ctrl+C exits the browser entirely
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
            | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                return Some(HistoryBrowserResult::Cancelled);
            }

            // Navigation still works in filter mode
            (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                self.move_cursor_up();
            }
            (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                self.move_cursor_down();
            }
            // PageUp/PageDown only (no Ctrl+B/F which conflict
            // with Emacs cursor movement in text input context)
            (KeyCode::PageUp, _) => {
                self.move_page_up();
            }
            (KeyCode::PageDown, _) => {
                self.move_page_down();
            }
            // Alt+Home/End: list navigation
            (KeyCode::Home, m) if m.contains(KeyModifiers::ALT) => {
                self.move_to_top();
            }
            (KeyCode::End, m) if m.contains(KeyModifiers::ALT) => {
                self.move_to_bottom();
            }
            // Plain Home/End: move cursor within filter input
            (KeyCode::Home, _) => {
                self.filter.cursor_pos = 0;
            }
            (KeyCode::End, _) => {
                self.filter.cursor_pos = self.filter.raw_query.chars().count();
            }
            (KeyCode::Tab, _) => {
                self.toggle_selection();
            }
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.select_all_visible();
            }
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.unselect_all();
            }

            // Backspace
            (KeyCode::Backspace, _) => {
                if self.filter.cursor_pos > 0
                    && let Some((byte_pos, _)) = self
                        .filter
                        .raw_query
                        .char_indices()
                        .nth(self.filter.cursor_pos - 1)
                {
                    self.filter.raw_query.remove(byte_pos);
                    self.filter.cursor_pos -= 1;
                    self.filter.reparse();
                    self.update_filter();
                }
            }

            // Delete
            (KeyCode::Delete, _) => {
                if let Some((byte_pos, _)) = self
                    .filter
                    .raw_query
                    .char_indices()
                    .nth(self.filter.cursor_pos)
                {
                    self.filter.raw_query.remove(byte_pos);
                    self.filter.reparse();
                    self.update_filter();
                }
            }

            // Cursor movement
            (KeyCode::Left, KeyModifiers::NONE) if self.filter.cursor_pos > 0 => {
                self.filter.cursor_pos -= 1;
            }
            (KeyCode::Right, KeyModifiers::NONE) => {
                let query_len = self.filter.raw_query.chars().count();
                if self.filter.cursor_pos < query_len {
                    self.filter.cursor_pos += 1;
                }
            }

            // Character input
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                let byte_pos = self
                    .filter
                    .raw_query
                    .char_indices()
                    .nth(self.filter.cursor_pos)
                    .map(|(i, _)| i)
                    .unwrap_or(self.filter.raw_query.len());
                self.filter.raw_query.insert(byte_pos, c);
                self.filter.cursor_pos += 1;
                self.filter.reparse();
                self.update_filter();
            }

            _ => {}
        }
        None
    }

    /// Normal mode: single-char keybindings work.
    fn handle_key_normal(&mut self, key: KeyEvent) -> Option<HistoryBrowserResult> {
        match (key.code, key.modifiers) {
            // Exit
            (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                return Some(HistoryBrowserResult::Cancelled);
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL)
            | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                return Some(HistoryBrowserResult::Cancelled);
            }

            // Enter filter mode
            (KeyCode::Char('/'), KeyModifiers::NONE) => {
                self.filter_active = true;
            }

            // Navigation - up
            (KeyCode::Up, _)
            | (KeyCode::Char('k'), KeyModifiers::NONE)
            | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                self.move_cursor_up();
            }

            // Navigation - down
            (KeyCode::Down, _)
            | (KeyCode::Char('j'), KeyModifiers::NONE)
            | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                self.move_cursor_down();
            }

            // Page up
            (KeyCode::PageUp, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                self.move_page_up();
            }

            // Page down
            (KeyCode::PageDown, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                self.move_page_down();
            }

            // Home / go to top
            (KeyCode::Home, _) | (KeyCode::Char('g'), KeyModifiers::NONE) => {
                self.move_to_top();
            }

            // End / go to bottom
            (KeyCode::End, _) | (KeyCode::Char('G'), KeyModifiers::SHIFT) => {
                self.move_to_bottom();
            }

            // Toggle selection
            (KeyCode::Tab, _) => {
                self.toggle_selection();
            }

            // Toggle selection and move down
            (KeyCode::Char(' '), KeyModifiers::NONE) => {
                self.toggle_selection();
                self.move_cursor_down();
            }

            // Select all visible
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.select_all_visible();
            }

            // Unselect all
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.unselect_all();
            }

            // Delete selected (show confirmation)
            (KeyCode::Char('d'), KeyModifiers::NONE) => {
                if self.selected_count() > 0 {
                    self.show_delete_dialog = true;
                } else {
                    self.feedback_message = Some("No items selected".to_string());
                }
            }

            // Copy and exit
            (KeyCode::Enter, _) => {
                if let Some(cmd) = self.current_command() {
                    let cmd = cmd.to_string();
                    if copy_to_clipboard(&cmd).is_ok() {
                        return Some(HistoryBrowserResult::Copied(cmd));
                    } else {
                        self.feedback_message = Some("Failed to copy".to_string());
                    }
                }
            }

            // Copy and stay
            (KeyCode::Char('y'), KeyModifiers::NONE) => {
                if let Some(cmd) = self.current_command() {
                    if copy_to_clipboard(cmd).is_ok() {
                        self.feedback_message = Some("Copied to clipboard".to_string());
                    } else {
                        self.feedback_message = Some("Failed to copy".to_string());
                    }
                }
            }

            _ => {}
        }
        None
    }

    /// Handle a mouse event. Returns `true` if a redraw is needed.
    fn handle_mouse(&mut self, kind: MouseEventKind) -> bool {
        match kind {
            MouseEventKind::ScrollUp => {
                self.move_cursor_up();
                true
            }
            MouseEventKind::ScrollDown => {
                self.move_cursor_down();
                true
            }
            _ => false,
        }
    }
}
