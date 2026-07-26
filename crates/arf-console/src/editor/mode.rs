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
//! - Auto-match should check what character follows the cursor
//!
//! # Solution
//!
//! We use a "shadow tracking" approach where the EditMode wrapper maintains
//! its own estimate of cursor position by observing the events it returns.
//! This state is then used to make decisions about how to handle certain keys.

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

    /// Get the character immediately before the cursor, if any.
    ///
    /// Returns `None` if cursor is at the beginning or if state is uncertain.
    #[allow(dead_code)] // Used in tests and kept for potential future use
    pub fn char_before_cursor(&self) -> Option<char> {
        if self.uncertain || self.cursor_pos == 0 {
            return None;
        }
        self.buffer.chars().nth(self.cursor_pos - 1)
    }

    /// Get the character immediately after the cursor, if any.
    ///
    /// Returns `None` if cursor is at the end or if state is uncertain.
    pub fn char_after_cursor(&self) -> Option<char> {
        if self.uncertain || self.cursor_pos >= self.buffer_len {
            return None;
        }
        self.buffer.chars().nth(self.cursor_pos)
    }

    /// Check if the cursor is positioned inside an empty bracket pair.
    ///
    /// Returns `true` if the character before cursor is an opening bracket
    /// and the character after cursor is the matching closing bracket.
    /// Returns `false` if state is uncertain or if the buffer contains newlines
    /// (multiline mode where state tracking may be unreliable).
    pub fn is_inside_empty_pair(&self) -> bool {
        if self.uncertain {
            return false;
        }
        // In multiline mode, our state tracking may be out of sync with
        // reedline's actual buffer (due to Enter handling in default mode).
        // Disable bracket delete in this case to prevent data loss.
        if self.buffer.contains('\n') {
            return false;
        }
        let Some(before) = self.char_before_cursor() else {
            return false;
        };
        let Some(after) = self.char_after_cursor() else {
            return false;
        };
        matches!(
            (before, after),
            ('(', ')') | ('[', ']') | ('{', '}') | ('"', '"') | ('\'', '\'') | ('`', '`')
        )
    }

    /// Check if the cursor is inside an unclosed quote of the given type.
    ///
    /// Returns `true` if there's an odd number of unescaped quote characters
    /// before the cursor position, indicating we're inside an open string.
    /// Returns `false` if state is uncertain.
    ///
    /// This is used to prevent auto-match from inserting a pair of quotes
    /// when we're already inside a string and just want to close it.
    pub fn cursor_in_quote(&self, quote_char: char) -> bool {
        if self.uncertain {
            return false;
        }

        // Get text before cursor
        let text_before: String = self.buffer.chars().take(self.cursor_pos).collect();

        // Count unescaped quotes
        let mut count = 0;
        let mut chars = text_before.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                // Skip next character (escaped)
                chars.next();
            } else if c == quote_char {
                count += 1;
            }
        }

        // Odd count means we're inside an unclosed quote
        count % 2 == 1
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
                // This is important for auto-match: if the user presses Right arrow
                // while a hint is shown, the hint gets completed and the buffer
                // changes from "pr" to "print(\"hello\")". Without marking uncertain,
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

/// Condition: cursor is at the end of the buffer.
///
/// This is useful for auto-match behavior where we only want to insert
/// the closing bracket when typing at the end, not when inserting in
/// the middle of existing text.
///
/// Note: For auto-matching, prefer `CursorAtEndOrBeforeClosing` which also
/// allows auto-matching before closing characters.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct CursorAtEnd;

impl KeyCondition for CursorAtEnd {
    fn check(&self, state: &EditorState) -> bool {
        state.cursor_at_end()
    }
}

/// Condition: cursor is inside an empty bracket pair.
///
/// Returns true when the cursor is positioned between matching brackets
/// with no content inside (e.g., `(|)`, `[|]`, `{|}`).
/// Returns false if the state is uncertain to avoid incorrect deletions.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub struct InsideEmptyPair;

impl KeyCondition for InsideEmptyPair {
    fn check(&self, state: &EditorState) -> bool {
        state.is_inside_empty_pair()
    }
}

/// Condition: cursor is NOT inside an empty bracket pair.
///
/// This is the negation of `InsideEmptyPair`, used for bracket auto-deletion
/// rules where we want to apply the replacement when inside an empty pair.
#[derive(Debug, Clone, Copy)]
pub struct NotInsideEmptyPair;

impl KeyCondition for NotInsideEmptyPair {
    fn check(&self, state: &EditorState) -> bool {
        !state.is_inside_empty_pair()
    }
}

/// Condition: cursor is at end of buffer OR before a closing character.
///
/// Closing characters are: `)`, `]`, `}`, `"`, `'`, `` ` ``
///
/// This allows auto-matching to work inside existing brackets/quotes:
/// - `(│)` + `"` → `("│")`
/// - `"│"` + `(` → `"(│)"`
/// - `foo│` + `(` → `foo(│)` (cursor at end)
#[derive(Debug, Clone, Copy)]
pub struct CursorAtEndOrBeforeClosing;

impl CursorAtEndOrBeforeClosing {
    /// Characters that are considered "closing" - auto-match can occur before these.
    const CLOSING_CHARS: [char; 6] = [')', ']', '}', '"', '\'', '`'];
}

impl KeyCondition for CursorAtEndOrBeforeClosing {
    fn check(&self, state: &EditorState) -> bool {
        // Cursor at end always allows auto-match
        if state.cursor_at_end() {
            return true;
        }

        // Check if the character after cursor is a closing character
        if let Some(char_after) = state.char_after_cursor() {
            return Self::CLOSING_CHARS.contains(&char_after);
        }

        // When uncertain (char_after_cursor returns None), fall back to false
        // to avoid incorrect auto-matching
        false
    }
}

/// Condition: cursor is at end OR before closing char, AND not inside an unclosed quote.
///
/// This is used for quote auto-matching (`"`, `'`, `` ` ``). When already inside
/// an unclosed string, typing the closing quote should just insert one quote
/// (not a pair), allowing the user to close the string.
///
/// Example:
/// - `"foo|` + `"` → `"foo"|` (just close the string, don't insert `""`)
/// - `foo|` + `"` → `foo"|"` (not in string, auto-match works)
pub struct CursorAtEndOrBeforeClosingAndNotInQuote {
    quote_char: char,
}

impl CursorAtEndOrBeforeClosingAndNotInQuote {
    pub fn new(quote_char: char) -> Self {
        Self { quote_char }
    }
}

impl KeyCondition for CursorAtEndOrBeforeClosingAndNotInQuote {
    fn check(&self, state: &EditorState) -> bool {
        // First check position (at end or before closing char)
        if !CursorAtEndOrBeforeClosing.check(state) {
            return false;
        }

        // Then check we're not inside an unclosed quote of this type
        !state.cursor_in_quote(self.quote_char)
    }
}

/// Condition: cursor is NOT before the specified character.
///
/// This is used for skip-over rules. When the cursor IS before the specified
/// character, the condition returns false, triggering the fallback (which is
/// MoveRight to skip over the character).
///
/// When the cursor is NOT before the character, the condition returns true,
/// allowing the original event to pass through to subsequent rules.
pub struct CursorNotBeforeChar {
    target_char: char,
}

impl CursorNotBeforeChar {
    pub fn new(target_char: char) -> Self {
        Self { target_char }
    }
}

impl KeyCondition for CursorNotBeforeChar {
    fn check(&self, state: &EditorState) -> bool {
        // Return true if NOT before the target char (allow pass-through)
        // Return false if cursor IS before target char (use fallback = MoveRight)
        if let Some(char_after) = state.char_after_cursor() {
            char_after != self.target_char
        } else {
            // At end of buffer or uncertain state - not before target char
            true
        }
    }
}

/// Matcher function type for conditional rules.
///
/// Uses a boxed closure to allow capturing variables (e.g., the specific
/// character to match in auto-match rules).
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

    /// Add multiple conditional rules.
    pub fn with_rules(mut self, rules: impl IntoIterator<Item = ConditionalRule>) -> Self {
        self.rules.extend(rules);
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

/// Create conditional rules for smart auto-match behavior.
///
/// Auto-match activates when the cursor is at the end of the buffer OR when
/// the character after the cursor is a closing character (`)`, `]`, `}`, `"`, `'`, `` ` ``).
/// When typing in the middle of existing text (not before a closing char), only
/// the opening character is inserted (no automatic closing bracket).
///
/// This follows the behavior of radian which uses `following_text(r"[,)}\]]|$")`
/// to check that the cursor is followed by a comma, closing bracket, or end of line.
///
/// Examples:
/// - `foo│` + `(` → `foo(│)` (cursor at end)
/// - `(│)` + `"` → `("│")` (cursor before `)`)
/// - `"│"` + `(` → `"(│)"` (cursor before `"`)
/// - `foo│bar` + `(` → `foo(│bar` (cursor before `b`, no auto-match)
/// - `"foo│` + `"` → `"foo"|` (inside unclosed string, just close it)
pub fn create_auto_match_rules() -> Vec<ConditionalRule> {
    // Define pairs: (opening char, pair string, is_quote)
    let pairs: [(char, &str, bool); 6] = [
        ('(', "()", false),
        ('[', "[]", false),
        ('{', "{}", false),
        ('"', r#""""#, true),
        ('\'', "''", true),
        ('`', "``", true),
    ];

    pairs
        .into_iter()
        .map(|(open_char, pair, is_quote)| {
            let pair_string = pair.to_string();

            // For quotes, use special condition that also checks if inside unclosed string
            let condition: Box<dyn KeyCondition> = if is_quote {
                Box::new(CursorAtEndOrBeforeClosingAndNotInQuote::new(open_char))
            } else {
                Box::new(CursorAtEndOrBeforeClosing)
            };

            ConditionalRule {
                // Match the auto-match event: InsertString(pair) + MoveLeft
                match_event: Box::new(move |event| {
                    matches!(
                        event,
                        ReedlineEvent::Edit(cmds)
                        if cmds.len() == 2
                            && matches!(&cmds[0], EditCommand::InsertString(s) if s == &pair_string)
                            && matches!(&cmds[1], EditCommand::MoveLeft { select: false })
                    )
                }),
                // Auto-match when cursor is at end OR before a closing character
                // (and for quotes: not inside an unclosed string)
                condition,
                // When in middle of text (not before closing char), just insert opening char
                fallback_event: ReedlineEvent::Edit(vec![EditCommand::InsertChar(open_char)]),
            }
        })
        .collect()
}

/// Create conditional rules for skipping over closing characters.
///
/// When typing a closing character (`'`, `"`, `` ` ``, `)`, `]`, `}`) and the
/// cursor is already positioned before the same character, the cursor should
/// just move right instead of inserting another character.
///
/// This complements the auto-match behavior: when auto-match inserts a pair
/// (e.g., `()`) and the cursor is between them, typing the closing character
/// should skip over the existing one rather than inserting a duplicate.
///
/// # Rule Order
///
/// These rules should be applied BEFORE auto-match rules so they have
/// priority when the cursor is before a closing character.
///
/// # Example
///
/// - `stop('Test error|')` + `'` → `stop('Test error'|)` (skip over `'`)
/// - `foo(|)` + `)` → `foo()|` (skip over `)`)
///
/// # Design Note
///
/// This matches radian's behavior where:
/// ```python
/// @handle("'", filter=... & following_text("^'"))
/// def _(event):
///     event.current_buffer.cursor_right()
/// ```
///
/// # Bracket vs Quote Handling
///
/// For brackets (`)`/`]`/`}`), the keybinding generates `InsertChar(close_char)`,
/// which is distinct from the opening bracket's `InsertString(pair) + MoveLeft`.
/// This allows skip-over rules to only trigger when the closing bracket key is
/// pressed, not when the opening bracket key is pressed.
///
/// For quotes (`"`/`'`/`` ` ``), the same character is used for both opening and
/// closing, so the keybinding always generates `InsertString(pair) + MoveLeft`.
/// The skip-over rule matches this event and checks if cursor is before the
/// same quote character.
pub fn create_skip_over_rules() -> Vec<ConditionalRule> {
    let mut rules = Vec::new();

    // Bracket closing characters: match InsertChar(close_char)
    // These are bound separately from opening brackets in add_auto_match_keybindings
    let bracket_closing_chars = [')', ']', '}'];
    for close_char in bracket_closing_chars {
        rules.push(ConditionalRule {
            // Match InsertChar for the closing bracket
            match_event: Box::new(move |event| {
                matches!(
                    event,
                    ReedlineEvent::Edit(cmds)
                    if cmds.len() == 1
                        && matches!(&cmds[0], EditCommand::InsertChar(c) if *c == close_char)
                )
            }),
            // Condition: cursor is NOT before the closing char
            // If cursor IS before it (condition=false), use fallback (MoveRight)
            condition: Box::new(CursorNotBeforeChar::new(close_char)),
            // Skip over the existing closing character
            fallback_event: ReedlineEvent::Edit(vec![EditCommand::MoveRight { select: false }]),
        });
    }

    // Quote characters: match InsertString(pair) + MoveLeft
    // These use the same character for open and close
    let quote_chars: [(char, &str); 3] = [('"', r#""""#), ('\'', "''"), ('`', "``")];
    for (quote_char, pair) in quote_chars {
        let pair_string = pair.to_string();
        rules.push(ConditionalRule {
            // Match the auto-match event for quotes
            match_event: Box::new(move |event| {
                matches!(
                    event,
                    ReedlineEvent::Edit(cmds)
                    if cmds.len() == 2
                        && matches!(&cmds[0], EditCommand::InsertString(s) if s == &pair_string)
                        && matches!(&cmds[1], EditCommand::MoveLeft { select: false })
                )
            }),
            // Condition: cursor is NOT before the quote char
            condition: Box::new(CursorNotBeforeChar::new(quote_char)),
            // Skip over the existing quote character
            fallback_event: ReedlineEvent::Edit(vec![EditCommand::MoveRight { select: false }]),
        });
    }

    rules
}

/// Create conditional rules for bracket pair auto-deletion.
///
/// When backspace is pressed inside an empty bracket pair (e.g., `(|)`),
/// both the opening and closing brackets are deleted together.
/// This complements the auto-match behavior that inserts pairs together.
///
/// The rule transforms a single Backspace into Backspace + Delete when
/// the cursor is inside an empty pair, effectively removing both brackets.
///
/// # Rule Logic
///
/// The ConditionalRule system uses:
/// - condition = true → keep original event
/// - condition = false → use fallback_event
///
/// So we use `NotInsideEmptyPair` as the condition:
/// - NotInsideEmptyPair = true (not in pair) → keep original Backspace
/// - NotInsideEmptyPair = false (in pair) → use Backspace + Delete
pub fn create_bracket_delete_rules() -> Vec<ConditionalRule> {
    vec![ConditionalRule {
        // Match a single Backspace command
        match_event: Box::new(|event| {
            matches!(
                event,
                ReedlineEvent::Edit(cmds)
                if cmds.len() == 1 && matches!(&cmds[0], EditCommand::Backspace)
            )
        }),
        // When NOT inside empty pair, keep the original Backspace
        // When inside empty pair (condition=false), use the fallback
        condition: Box::new(NotInsideEmptyPair),
        // Delete both brackets: Backspace removes opening, Delete removes closing
        fallback_event: ReedlineEvent::Edit(vec![EditCommand::Backspace, EditCommand::Delete]),
    }]
}

#[cfg(test)]
mod tests;
