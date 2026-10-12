//! Shadow tracking of the editor buffer and cursor.

use reedline::{EditCommand, ReedlineEvent};
use std::sync::{Arc, Mutex};

/// Editor state that can be shared and tracked across components.
///
/// This represents our "shadow" view of the editor's state, updated
/// by observing the events we return from `parse_event()`.
#[derive(Debug, Clone, Default)]
pub struct EditorState {
    /// Estimated cursor position (0-indexed from start of line).
    pub cursor_pos: usize,
    /// Estimated buffer length.
    pub buffer_len: usize,
    /// Shadow copy of buffer content for character inspection.
    pub buffer: String,
    /// Whether the shadow state may be out of sync with actual buffer.
    /// When true, rules requiring exact buffer content should fall back to safe defaults.
    pub uncertain: bool,
}

impl EditorState {
    /// Create a new editor state at the start of input.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset state for a new line of input.
    pub fn reset(&mut self) {
        self.cursor_pos = 0;
        self.buffer_len = 0;
        self.buffer.clear();
        self.uncertain = false;
    }

    /// Check if cursor is at the beginning of the line.
    pub fn cursor_at_begin(&self) -> bool {
        self.cursor_pos == 0
    }

    /// Check if the buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.buffer_len == 0
    }

    /// Check if cursor is at the end of the buffer.
    pub fn cursor_at_end(&self) -> bool {
        self.cursor_pos == self.buffer_len
    }

    /// Convert a character position to a byte position in the buffer.
    ///
    /// This is necessary because Rust strings are UTF-8 encoded, so
    /// multi-byte characters need proper handling.
    fn char_to_byte_pos(&self, char_pos: usize) -> usize {
        self.buffer
            .char_indices()
            .nth(char_pos)
            .map(|(i, _)| i)
            .unwrap_or(self.buffer.len())
    }

    /// Update state based on a ReedlineEvent.
    ///
    /// This is the core of our shadow tracking - we observe the events
    /// we're returning and update our state estimate accordingly.
    pub fn update_from_event(&mut self, event: &ReedlineEvent) {
        match event {
            ReedlineEvent::Edit(commands) => {
                for cmd in commands {
                    self.update_from_edit_command(cmd);
                }
            }
            ReedlineEvent::Multiple(events) => {
                for e in events {
                    self.update_from_event(e);
                }
            }
            ReedlineEvent::UntilFound(events) => {
                // UntilFound executes until one succeeds - we can't know which
                // one will succeed.
                //
                // HistoryHintComplete/HistoryHintWordComplete can change the buffer
                // significantly (inserting the entire hint text). When these events
                // are in the list and might succeed, we mark state as uncertain
                // because the actual buffer content after completion is unknown.
                //
                // However, HistoryHintComplete only succeeds when:
                // 1. A hint is visible (we can't know this)
                // 2. Cursor is at the end of the buffer (we CAN check this)
                //
                // If cursor is not at end, HistoryHintComplete will fail and
                // fall through to subsequent events (like Right), which we can track.
                // A preceding MenuLeft/MenuRight may instead be consumed by an
                // active menu, so cursor movement after it is ambiguous. Mark the
                // shadow state uncertain; CombinedHighlighter will resync it from
                // the actual line and cursor on the next repaint.
                //
                // If the user presses Right arrow while a hint is shown, the hint
                // gets completed and the buffer changes from "pr" to
                // `print("hello")`. Without marking uncertain,
                // the shadow state would incorrectly think cursor just moved right.
                for e in events {
                    if matches!(
                        e,
                        ReedlineEvent::HistoryHintComplete | ReedlineEvent::HistoryHintWordComplete
                    ) {
                        // Only mark uncertain if hint completion could succeed:
                        // 1. Cursor must be at buffer end
                        // 2. Buffer must be non-empty (hinter requires min_chars >= 1)
                        //
                        // If buffer is empty, no hint can be shown, so HistoryHintComplete
                        // will definitely fail.
                        if self.cursor_at_end() && self.buffer_len > 0 {
                            self.uncertain = true;
                            return;
                        }
                        // Hint completion will fail - continue to track subsequent events
                        break;
                    }
                }

                // Look for navigation events which are most likely to succeed
                // and affect cursor position.
                for e in events {
                    match e {
                        ReedlineEvent::MenuLeft | ReedlineEvent::MenuRight => {
                            self.uncertain = true;
                            return;
                        }
                        // Navigation events that affect cursor position
                        ReedlineEvent::Left | ReedlineEvent::Right => {
                            self.update_from_event(e);
                            return;
                        }
                        ReedlineEvent::Edit(_) => {
                            self.update_from_event(e);
                            return;
                        }
                        _ => {}
                    }
                }
                // If no navigation event found, try the first one
                if let Some(first) = events.first() {
                    self.update_from_event(first);
                }
            }
            ReedlineEvent::Submit | ReedlineEvent::SubmitOrNewline | ReedlineEvent::Enter => {
                // Mark state as uncertain rather than resetting.
                // Enter can either submit or insert a newline depending on validation.
                // If validation returns Incomplete, reedline will add a newline
                // without going through parse_event, making our state stale.
                // By marking uncertain, all condition checks will use safe defaults.
                self.uncertain = true;
            }
            // Navigation events (used in UntilFound for arrow keys)
            ReedlineEvent::Left if self.cursor_pos > 0 => {
                self.cursor_pos -= 1;
            }
            ReedlineEvent::Right if self.cursor_pos < self.buffer_len => {
                self.cursor_pos += 1;
            }
            ReedlineEvent::Up | ReedlineEvent::Down => {
                // History navigation - we can't know the new buffer state.
                // Mark as uncertain since the buffer content will change.
                self.uncertain = true;
            }
            // Menu events, etc. don't change buffer state
            _ => {}
        }
    }

    /// Update state based on an EditCommand.
    fn update_from_edit_command(&mut self, cmd: &EditCommand) {
        match cmd {
            EditCommand::InsertChar(c) => {
                // Insert char at cursor position
                let byte_pos = self.char_to_byte_pos(self.cursor_pos);
                self.buffer.insert(byte_pos, *c);
                self.cursor_pos += 1;
                self.buffer_len += 1;
            }
            EditCommand::InsertString(s) => {
                let len = s.chars().count();
                let byte_pos = self.char_to_byte_pos(self.cursor_pos);
                self.buffer.insert_str(byte_pos, s);
                self.cursor_pos += len;
                self.buffer_len += len;
            }
            EditCommand::InsertNewline => {
                // Newline in multiline mode - cursor goes to start of new line
                // For our purposes, we can treat this as extending the buffer
                let byte_pos = self.char_to_byte_pos(self.cursor_pos);
                self.buffer.insert(byte_pos, '\n');
                self.cursor_pos += 1;
                self.buffer_len += 1;
            }
            EditCommand::Backspace => {
                if self.cursor_pos > 0 {
                    // Remove char before cursor
                    let remove_pos = self.cursor_pos - 1;
                    let byte_start = self.char_to_byte_pos(remove_pos);
                    let byte_end = self.char_to_byte_pos(self.cursor_pos);
                    self.buffer.drain(byte_start..byte_end);
                    self.cursor_pos -= 1;
                    self.buffer_len -= 1;
                }
            }
            EditCommand::Delete => {
                // Delete char at cursor - cursor stays, buffer shrinks
                if self.cursor_pos < self.buffer_len {
                    let byte_start = self.char_to_byte_pos(self.cursor_pos);
                    let byte_end = self.char_to_byte_pos(self.cursor_pos + 1);
                    self.buffer.drain(byte_start..byte_end);
                    self.buffer_len -= 1;
                }
            }
            EditCommand::MoveLeft { .. } => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            EditCommand::MoveRight { .. } => {
                if self.cursor_pos < self.buffer_len {
                    self.cursor_pos += 1;
                }
            }
            EditCommand::MoveToStart { .. } | EditCommand::MoveToLineStart { .. } => {
                self.cursor_pos = 0;
            }
            EditCommand::MoveToEnd { .. } | EditCommand::MoveToLineEnd { .. } => {
                self.cursor_pos = self.buffer_len;
            }
            EditCommand::Clear => {
                self.reset();
            }
            EditCommand::CutFromStart => {
                // Cut from start to cursor
                let byte_end = self.char_to_byte_pos(self.cursor_pos);
                self.buffer.drain(0..byte_end);
                self.buffer_len = self.buffer_len.saturating_sub(self.cursor_pos);
                self.cursor_pos = 0;
            }
            EditCommand::CutToEnd | EditCommand::CutToLineEnd => {
                // Cut from cursor to end
                let byte_start = self.char_to_byte_pos(self.cursor_pos);
                self.buffer.truncate(byte_start);
                self.buffer_len = self.cursor_pos;
            }
            EditCommand::CutWordLeft | EditCommand::CutWordRight => {
                // Word operations are complex - mark as uncertain
                // Position/length updates are approximate anyway
                self.uncertain = true;
                if matches!(cmd, EditCommand::CutWordLeft) {
                    let removed = self.cursor_pos.min(5);
                    self.cursor_pos -= removed;
                    self.buffer_len = self.buffer_len.saturating_sub(removed);
                } else {
                    let remaining = self.buffer_len - self.cursor_pos;
                    let removed = remaining.min(5);
                    self.buffer_len -= removed;
                }
            }
            // For other commands, mark as uncertain since we can't track buffer changes
            _ => {
                self.uncertain = true;
            }
        }
    }
}

/// A shared reference to editor state.
pub type EditorStateRef = Arc<Mutex<EditorState>>;

/// Create a new shared editor state reference.
pub fn new_editor_state_ref() -> EditorStateRef {
    Arc::new(Mutex::new(EditorState::new()))
}
