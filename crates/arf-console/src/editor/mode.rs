//! Custom EditMode wrapper for conditional keybinding behavior.
//!
//! This module provides infrastructure to implement buffer-aware keybindings
//! that can check the current cursor position or buffer content before
//! deciding what action to take.
//!
//! # Problem
//!
//! reedline's `EditMode::parse_event()` only receives the raw key event,
//! not the current buffer content. This prevents conditional keybinding
//! behavior like:
//! - ':' should only trigger completion when at line position 0
//! - Other stateful shortcuts can inspect the tracked buffer and cursor
//!
//! # Solution
//!
//! We use a "shadow tracking" approach where the EditMode wrapper maintains
//! an estimate of cursor position and buffer state by observing returned
//! events. This state supports shortcuts and completion behavior that need
//! context unavailable to `EditMode::parse_event` itself.

use reedline::{EditCommand, EditMode, PromptEditMode, ReedlineEvent};
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
                //
                // If the user presses Right arrow while a hint is shown, the hint
                // gets completed and the buffer changes from "pr" to
                // "print(\"hello\")". Without marking uncertain,
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

/// Condition that can be checked before processing a keybinding.
pub trait KeyCondition: Send + Sync {
    /// Check if the condition is met given the current editor state.
    fn check(&self, state: &EditorState) -> bool;
}

/// Condition: cursor is at the beginning of the line.
#[derive(Debug, Clone, Copy)]
pub struct CursorAtBegin;

impl KeyCondition for CursorAtBegin {
    fn check(&self, state: &EditorState) -> bool {
        state.cursor_at_begin()
    }
}

/// Condition: buffer is empty.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct BufferEmpty;

impl KeyCondition for BufferEmpty {
    fn check(&self, state: &EditorState) -> bool {
        state.is_empty()
    }
}

/// Condition: buffer is known to be empty (empty and not uncertain).
#[derive(Debug, Clone, Copy)]
pub struct BufferKnownEmpty;

impl KeyCondition for BufferKnownEmpty {
    fn check(&self, state: &EditorState) -> bool {
        state.is_empty() && !state.uncertain
    }
}

/// Matcher function type for conditional rules.
///
/// Uses a boxed closure to allow capturing values used by a rule.
pub type EventMatcher = Box<dyn Fn(&ReedlineEvent) -> bool + Send + Sync>;

/// A conditional keybinding rule.
///
/// Specifies that when a certain key produces a specific event,
/// the event should be modified if a condition is not met.
pub struct ConditionalRule {
    /// The original event pattern to match (boxed to allow captured variables).
    pub match_event: EventMatcher,
    /// Condition that must be true for the original event to be kept.
    pub condition: Box<dyn KeyCondition>,
    /// Event to use instead if condition is not met.
    pub fallback_event: ReedlineEvent,
}

/// A wrapper around an EditMode that applies conditional rules.
///
/// This wrapper intercepts `parse_event()` calls, checks conditions
/// against the current editor state, and potentially modifies the
/// returned event.
pub struct ConditionalEditMode<E: EditMode> {
    inner: E,
    state: EditorStateRef,
    rules: Vec<ConditionalRule>,
    /// Minimum characters to trigger automatic completion display.
    /// When Some(n), completion menu is shown after typing n or more characters.
    completion_min_chars: Option<usize>,
    /// Use tree-sitter based word navigation for R code.
    /// When enabled, MoveWordLeft/MoveWordRight will use R token boundaries.
    tree_sitter_word_nav: bool,
}

impl<E: EditMode> ConditionalEditMode<E> {
    /// Create a new conditional edit mode wrapper.
    pub fn new(inner: E, state: EditorStateRef) -> Self {
        Self {
            inner,
            state,
            rules: Vec::new(),
            completion_min_chars: None,
            tree_sitter_word_nav: false,
        }
    }

    /// Set the minimum characters to trigger automatic completion display.
    pub fn with_completion_min_chars(mut self, min_chars: Option<usize>) -> Self {
        self.completion_min_chars = min_chars;
        self
    }

    /// Enable tree-sitter based word navigation.
    ///
    /// When enabled, `MoveWordLeft` and `MoveWordRight` will use R token
    /// boundaries instead of unicode word boundaries. This allows jumping
    /// over operators like `|>`, `<-`, `%>%` as single units.
    pub fn with_tree_sitter_word_nav(mut self, enabled: bool) -> Self {
        self.tree_sitter_word_nav = enabled;
        self
    }

    /// Add a conditional rule.
    pub fn with_rule(mut self, rule: ConditionalRule) -> Self {
        self.rules.push(rule);
        self
    }

    /// Apply conditional rules to an event.
    fn apply_rules(&self, event: ReedlineEvent) -> ReedlineEvent {
        let state = self.state.lock().unwrap();

        for rule in &self.rules {
            if (rule.match_event)(&event) && !rule.condition.check(&state) {
                return rule.fallback_event.clone();
            }
        }

        event
    }

    /// Check if an event is a word movement command and handle it with tree-sitter.
    ///
    /// Returns `Some(event)` if the event was handled, `None` otherwise.
    fn handle_tree_sitter_word_nav(&self, event: &ReedlineEvent) -> Option<ReedlineEvent> {
        use super::word_nav::{token_left_position, token_right_position};

        if !self.tree_sitter_word_nav {
            return None;
        }

        let state = self.state.lock().unwrap();

        // Don't use tree-sitter if state is uncertain
        if state.uncertain {
            return None;
        }

        match event {
            ReedlineEvent::Edit(commands) if commands.len() == 1 => match &commands[0] {
                EditCommand::MoveWordLeft { select } => {
                    let target = token_left_position(&state.buffer, state.cursor_pos);
                    Some(Self::create_move_event(state.cursor_pos, target, *select))
                }
                EditCommand::MoveWordRight { select } => {
                    let target = token_right_position(&state.buffer, state.cursor_pos);
                    Some(Self::create_move_event(state.cursor_pos, target, *select))
                }
                _ => None,
            },
            // Handle UntilFound containing word movement (e.g., Ctrl+Right with hint completion)
            ReedlineEvent::UntilFound(events) => {
                // Check if any of the events is a word movement
                for (i, e) in events.iter().enumerate() {
                    if let ReedlineEvent::Edit(commands) = e
                        && commands.len() == 1
                    {
                        if let EditCommand::MoveWordRight { select } = &commands[0] {
                            // Replace the word movement event with tree-sitter version
                            let target = token_right_position(&state.buffer, state.cursor_pos);
                            let move_event =
                                Self::create_move_event(state.cursor_pos, target, *select);

                            // Rebuild UntilFound with the replacement
                            let mut new_events = events.clone();
                            new_events[i] = move_event;
                            return Some(ReedlineEvent::UntilFound(new_events));
                        }
                        if let EditCommand::MoveWordLeft { select } = &commands[0] {
                            let target = token_left_position(&state.buffer, state.cursor_pos);
                            let move_event =
                                Self::create_move_event(state.cursor_pos, target, *select);

                            let mut new_events = events.clone();
                            new_events[i] = move_event;
                            return Some(ReedlineEvent::UntilFound(new_events));
                        }
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// Create a movement event to go from current position to target position.
    fn create_move_event(current: usize, target: usize, select: bool) -> ReedlineEvent {
        if current == target {
            return ReedlineEvent::None;
        }

        if target < current {
            // Move left
            let diff = current - target;
            let commands: Vec<EditCommand> =
                std::iter::repeat_n(EditCommand::MoveLeft { select }, diff).collect();
            ReedlineEvent::Edit(commands)
        } else {
            // Move right
            let diff = target - current;
            let commands: Vec<EditCommand> =
                std::iter::repeat_n(EditCommand::MoveRight { select }, diff).collect();
            ReedlineEvent::Edit(commands)
        }
    }
}

impl<E: EditMode> EditMode for ConditionalEditMode<E> {
    fn parse_event(&mut self, event: reedline::ReedlineRawEvent) -> ReedlineEvent {
        // Get the event from the inner edit mode
        let inner_event = self.inner.parse_event(event);

        // Apply our conditional rules
        let rules_event = self.apply_rules(inner_event);

        // Apply tree-sitter word navigation if enabled
        let final_event = self
            .handle_tree_sitter_word_nav(&rules_event)
            .unwrap_or(rules_event);

        // Update our shadow state based on the event we're returning
        {
            let mut state = self.state.lock().unwrap();
            state.update_from_event(&final_event);
        }

        // Auto-trigger completion if configured and conditions are met
        if let Some(min_chars) = self.completion_min_chars {
            let state = self.state.lock().unwrap();
            // Only trigger if:
            // - Buffer has enough characters
            // - State is not uncertain (we know the actual buffer state)
            // - The event was a character insertion
            if state.buffer_len >= min_chars
                && !state.uncertain
                && is_character_insert(&final_event)
            {
                return ReedlineEvent::Multiple(vec![
                    final_event,
                    ReedlineEvent::Menu("completion_menu".to_string()),
                ]);
            }
        }

        final_event
    }

    fn edit_mode(&self) -> PromptEditMode {
        self.inner.edit_mode()
    }
}

/// Check if a ReedlineEvent represents a character insertion.
///
/// Returns true for events that insert text into the buffer,
/// which should trigger auto-completion when enabled.
fn is_character_insert(event: &ReedlineEvent) -> bool {
    match event {
        ReedlineEvent::Edit(commands) => commands.iter().any(|cmd| {
            matches!(
                cmd,
                EditCommand::InsertChar(_) | EditCommand::InsertString(_)
            )
        }),
        ReedlineEvent::Multiple(events) => events.iter().any(is_character_insert),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
