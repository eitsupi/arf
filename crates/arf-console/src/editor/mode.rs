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
            ReedlineEvent::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            ReedlineEvent::Right => {
                if self.cursor_pos < self.buffer_len {
                    self.cursor_pos += 1;
                }
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
            if (rule.match_event)(&event) {
                if !rule.condition.check(&state) {
                    return rule.fallback_event.clone();
                }
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
            ReedlineEvent::Edit(commands) if commands.len() == 1 => {
                match &commands[0] {
                    EditCommand::MoveWordLeft { select } => {
                        let target = token_left_position(&state.buffer, state.cursor_pos);
                        Some(Self::create_move_event(state.cursor_pos, target, *select))
                    }
                    EditCommand::MoveWordRight { select } => {
                        let target = token_right_position(&state.buffer, state.cursor_pos);
                        Some(Self::create_move_event(state.cursor_pos, target, *select))
                    }
                    _ => None,
                }
            }
            // Handle UntilFound containing word movement (e.g., Ctrl+Right with hint completion)
            ReedlineEvent::UntilFound(events) => {
                // Check if any of the events is a word movement
                for (i, e) in events.iter().enumerate() {
                    if let ReedlineEvent::Edit(commands) = e {
                        if commands.len() == 1 {
                            if let EditCommand::MoveWordRight { select } = &commands[0] {
                                // Replace the word movement event with tree-sitter version
                                let target =
                                    token_right_position(&state.buffer, state.cursor_pos);
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
                std::iter::repeat(EditCommand::MoveLeft { select })
                    .take(diff)
                    .collect();
            ReedlineEvent::Edit(commands)
        } else {
            // Move right
            let diff = target - current;
            let commands: Vec<EditCommand> =
                std::iter::repeat(EditCommand::MoveRight { select })
                    .take(diff)
                    .collect();
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
        ReedlineEvent::Edit(commands) => {
            commands.iter().any(|cmd| {
                matches!(
                    cmd,
                    EditCommand::InsertChar(_) | EditCommand::InsertString(_)
                )
            })
        }
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
pub fn create_skip_over_rules() -> Vec<ConditionalRule> {
    // All closing characters that can be skipped over
    let closing_chars: [(char, &str); 6] = [
        (')', "()"),
        (']', "[]"),
        ('}', "{}"),
        ('"', r#""""#),
        ('\'', "''"),
        ('`', "``"),
    ];

    closing_chars
        .into_iter()
        .map(|(close_char, pair)| {
            let pair_string = pair.to_string();

            ConditionalRule {
                // Match the auto-match event for this character's pair
                // (The keybinding always generates InsertString(pair) + MoveLeft)
                match_event: Box::new(move |event| {
                    matches!(
                        event,
                        ReedlineEvent::Edit(cmds)
                        if cmds.len() == 2
                            && matches!(&cmds[0], EditCommand::InsertString(s) if s == &pair_string)
                            && matches!(&cmds[1], EditCommand::MoveLeft { select: false })
                    )
                }),
                // Condition: cursor is NOT before the closing char
                // If cursor IS before it (condition=false), use fallback (MoveRight)
                condition: Box::new(CursorNotBeforeChar::new(close_char)),
                // Skip over the existing closing character
                fallback_event: ReedlineEvent::Edit(vec![EditCommand::MoveRight { select: false }]),
            }
        })
        .collect()
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
mod tests {
    use super::*;

    #[test]
    fn test_editor_state_initial() {
        let state = EditorState::new();
        assert_eq!(state.cursor_pos, 0);
        assert_eq!(state.buffer_len, 0);
        assert!(state.cursor_at_begin());
        assert!(state.is_empty());
    }

    #[test]
    fn test_editor_state_insert_char() {
        let mut state = EditorState::new();

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
        assert_eq!(state.cursor_pos, 1);
        assert_eq!(state.buffer_len, 1);
        assert!(!state.cursor_at_begin());
        assert!(!state.is_empty());

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
        assert_eq!(state.cursor_pos, 2);
        assert_eq!(state.buffer_len, 2);
    }

    #[test]
    fn test_editor_state_insert_string() {
        let mut state = EditorState::new();

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertString(
            "hello".to_string(),
        )]));
        assert_eq!(state.cursor_pos, 5);
        assert_eq!(state.buffer_len, 5);
    }

    #[test]
    fn test_editor_state_backspace() {
        let mut state = EditorState::new();

        // Insert some text
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertString(
            "abc".to_string(),
        )]));
        assert_eq!(state.cursor_pos, 3);

        // Backspace
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));
        assert_eq!(state.cursor_pos, 2);
        assert_eq!(state.buffer_len, 2);

        // Backspace at beginning should be no-op
        state.cursor_pos = 0;
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_editor_state_move() {
        let mut state = EditorState::new();
        state.buffer_len = 5;
        state.cursor_pos = 2;

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::MoveLeft {
            select: false,
        }]));
        assert_eq!(state.cursor_pos, 1);

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::MoveRight {
            select: false,
        }]));
        assert_eq!(state.cursor_pos, 2);

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::MoveToStart {
            select: false,
        }]));
        assert_eq!(state.cursor_pos, 0);

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::MoveToEnd {
            select: false,
        }]));
        assert_eq!(state.cursor_pos, 5);
    }

    #[test]
    fn test_editor_state_uncertain_on_submit() {
        let mut state = EditorState::new();
        state.buffer = "hello".to_string();
        state.cursor_pos = 5;
        state.buffer_len = 5;
        assert!(!state.uncertain);

        // Submit should mark state as uncertain, not reset it.
        // This is because if validation returns Incomplete, reedline will add
        // a newline without going through parse_event, making our state stale.
        state.update_from_event(&ReedlineEvent::Submit);
        assert!(state.uncertain);

        // Buffer and cursor are preserved (but uncertain, so won't be trusted)
        assert_eq!(state.cursor_pos, 5);
        assert_eq!(state.buffer_len, 5);
    }

    #[test]
    fn test_editor_state_multiple_events() {
        let mut state = EditorState::new();

        // Multiple events in one
        state.update_from_event(&ReedlineEvent::Multiple(vec![
            ReedlineEvent::Edit(vec![EditCommand::InsertChar(':')]),
            ReedlineEvent::Menu("completion_menu".to_string()),
        ]));

        // Only the Edit affects state
        assert_eq!(state.cursor_pos, 1);
        assert_eq!(state.buffer_len, 1);
    }

    #[test]
    fn test_cursor_at_begin_condition() {
        let condition = CursorAtBegin;

        let mut state = EditorState::new();
        assert!(condition.check(&state));

        state.cursor_pos = 1;
        assert!(!condition.check(&state));
    }

    #[test]
    fn test_buffer_empty_condition() {
        let condition = BufferEmpty;

        let mut state = EditorState::new();
        assert!(condition.check(&state));

        state.buffer_len = 1;
        assert!(!condition.check(&state));
    }

    #[test]
    fn test_cursor_at_end_condition() {
        let condition = CursorAtEnd;

        // Empty buffer: cursor at end
        let mut state = EditorState::new();
        assert!(state.cursor_at_end());
        assert!(condition.check(&state));

        // Buffer with content, cursor at end
        state.buffer_len = 5;
        state.cursor_pos = 5;
        assert!(state.cursor_at_end());
        assert!(condition.check(&state));

        // Buffer with content, cursor in middle
        state.cursor_pos = 2;
        assert!(!state.cursor_at_end());
        assert!(!condition.check(&state));

        // Buffer with content, cursor at beginning
        state.cursor_pos = 0;
        assert!(!state.cursor_at_end());
        assert!(!condition.check(&state));
    }

    #[test]
    fn test_auto_match_rules_created() {
        let rules = create_auto_match_rules();

        // Should have 6 rules (for (, [, {, ", ', `)
        assert_eq!(rules.len(), 6);
    }

    #[test]
    fn test_auto_match_rule_matches_paren() {
        let rules = create_auto_match_rules();
        let paren_rule = &rules[0]; // '(' rule

        // Should match the auto-match event for '('
        let match_event = ReedlineEvent::Edit(vec![
            EditCommand::InsertString("()".to_string()),
            EditCommand::MoveLeft { select: false },
        ]);
        assert!((paren_rule.match_event)(&match_event));

        // Should not match other events
        let other_event = ReedlineEvent::Edit(vec![EditCommand::InsertChar('(')]);
        assert!(!(paren_rule.match_event)(&other_event));

        // Should not match different pair
        let bracket_event = ReedlineEvent::Edit(vec![
            EditCommand::InsertString("[]".to_string()),
            EditCommand::MoveLeft { select: false },
        ]);
        assert!(!(paren_rule.match_event)(&bracket_event));
    }

    #[test]
    fn test_auto_match_condition_cursor_at_end() {
        let rules = create_auto_match_rules();
        let rule = &rules[0]; // '(' rule

        // Cursor at end: condition should pass
        let mut state = EditorState::new();
        state.buffer_len = 5;
        state.cursor_pos = 5;
        assert!(rule.condition.check(&state));

        // Cursor not at end: condition should fail
        state.cursor_pos = 2;
        assert!(!rule.condition.check(&state));
    }

    #[test]
    fn test_auto_match_fallback_event() {
        let rules = create_auto_match_rules();

        // Check fallback events for each pair
        let expected_fallbacks = ['(', '[', '{', '"', '\'', '`'];
        for (rule, expected_char) in rules.iter().zip(expected_fallbacks.iter()) {
            match &rule.fallback_event {
                ReedlineEvent::Edit(cmds) => {
                    assert_eq!(cmds.len(), 1);
                    match &cmds[0] {
                        EditCommand::InsertChar(c) => assert_eq!(c, expected_char),
                        _ => panic!("Expected InsertChar"),
                    }
                }
                _ => panic!("Expected Edit event"),
            }
        }
    }

    /// Test the full flow: simulate typing characters then check auto-match behavior
    #[test]
    fn test_auto_match_full_flow_at_end() {
        let state_ref = new_editor_state_ref();
        let rules = create_auto_match_rules();

        // Simulate typing "abc" - update state manually as if these events happened
        {
            let mut state = state_ref.lock().unwrap();
            // After typing 'a'
            state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
            // After typing 'b'
            state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
            // After typing 'c'
            state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('c')]));
            // State should now be: cursor_pos=3, buffer_len=3
            assert_eq!(state.cursor_pos, 3);
            assert_eq!(state.buffer_len, 3);
            assert!(state.cursor_at_end());
        }

        // Now check if the paren rule condition passes
        let paren_rule = &rules[0];
        {
            let state = state_ref.lock().unwrap();
            // cursor_at_end should be true
            assert!(state.cursor_at_end());
            // condition should pass
            assert!(paren_rule.condition.check(&state));
        }
    }

    /// Test that auto-match is blocked when cursor is not at end
    #[test]
    fn test_auto_match_blocked_when_not_at_end() {
        let state_ref = new_editor_state_ref();
        let rules = create_auto_match_rules();

        // Simulate typing "abc" then moving left
        {
            let mut state = state_ref.lock().unwrap();
            state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
            state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
            state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('c')]));
            // Move left
            state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::MoveLeft {
                select: false,
            }]));
            // State should now be: cursor_pos=2, buffer_len=3
            assert_eq!(state.cursor_pos, 2);
            assert_eq!(state.buffer_len, 3);
            assert!(!state.cursor_at_end());
        }

        // Now check if the paren rule condition fails (cursor not at end)
        let paren_rule = &rules[0];
        {
            let state = state_ref.lock().unwrap();
            assert!(!state.cursor_at_end());
            assert!(!paren_rule.condition.check(&state));
        }
    }

    /// Test that ReedlineEvent::Right is handled in UntilFound when cursor NOT at end
    ///
    /// In reedline, Right arrow binding is:
    /// `UntilFound([HistoryHintComplete, MenuRight, Right])`
    ///
    /// When cursor is NOT at buffer end, HistoryHintComplete will fail,
    /// so we can safely track the Right movement.
    #[test]
    fn test_until_found_right_updates_cursor() {
        let mut state = EditorState::new();

        // Simulate typing "abc"
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('c')]));
        assert_eq!(state.cursor_pos, 3);
        assert_eq!(state.buffer_len, 3);

        // Move left so cursor is NOT at end
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::MoveLeft {
            select: false,
        }]));
        assert_eq!(state.cursor_pos, 2);
        assert!(!state.cursor_at_end());

        // Simulate right arrow key (UntilFound with Right)
        // Since cursor is NOT at end, HistoryHintComplete will fail,
        // so the Right event will succeed and we track it.
        state.update_from_event(&ReedlineEvent::UntilFound(vec![
            ReedlineEvent::HistoryHintComplete,
            ReedlineEvent::MenuRight,
            ReedlineEvent::Right,
        ]));

        // After Right, cursor should be at position 3 (back at end)
        assert_eq!(state.cursor_pos, 3);
        assert!(state.cursor_at_end());
        assert!(!state.uncertain); // State is still certain
    }

    /// Test that Right arrow at buffer end marks state as uncertain
    ///
    /// When cursor is at buffer end and Right arrow is pressed, HistoryHintComplete
    /// might succeed (if a hint is visible), changing the buffer significantly.
    /// Since we can't know if a hint was completed, we mark state as uncertain.
    #[test]
    fn test_until_found_right_at_end_marks_uncertain() {
        let mut state = EditorState::new();

        // Simulate typing "abc" - cursor at end
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('c')]));
        assert_eq!(state.cursor_pos, 3);
        assert!(state.cursor_at_end());
        assert!(!state.uncertain);

        // Simulate right arrow key while cursor at end
        // HistoryHintComplete might succeed, so state becomes uncertain
        state.update_from_event(&ReedlineEvent::UntilFound(vec![
            ReedlineEvent::HistoryHintComplete,
            ReedlineEvent::MenuRight,
            ReedlineEvent::Right,
        ]));

        // State should be marked as uncertain
        assert!(state.uncertain);
    }

    /// Test that Right arrow on EMPTY buffer does NOT mark uncertain
    ///
    /// HistoryHintComplete requires min_chars >= 1 to show a hint.
    /// On empty buffer, no hint can be shown, so HistoryHintComplete will fail.
    /// We should NOT mark uncertain in this case.
    #[test]
    fn test_until_found_right_on_empty_buffer_not_uncertain() {
        let mut state = EditorState::new();

        // Empty buffer: cursor_pos=0, buffer_len=0
        assert_eq!(state.cursor_pos, 0);
        assert_eq!(state.buffer_len, 0);
        assert!(state.cursor_at_end()); // 0 == 0
        assert!(!state.uncertain);

        // Simulate right arrow key on empty buffer
        state.update_from_event(&ReedlineEvent::UntilFound(vec![
            ReedlineEvent::HistoryHintComplete,
            ReedlineEvent::MenuRight,
            ReedlineEvent::Right,
        ]));

        // State should NOT be uncertain (no hint possible on empty buffer)
        assert!(!state.uncertain);
        // Cursor should still be at 0 (can't move right on empty buffer)
        assert_eq!(state.cursor_pos, 0);
    }

    /// Regression test: auto-match should work inside braces
    ///
    /// Scenario: `{|}` -> type `(` -> `{(|)}`
    /// This tests that the shadow state correctly tracks cursor position
    /// inside bracket pairs, allowing nested auto-match to work.
    #[test]
    fn test_auto_match_inside_braces_regression() {
        let rules = create_auto_match_rules();
        let paren_rule = &rules[0]; // '(' rule

        let mut state = EditorState::new();

        // Type `{` with auto-match: inserts "{}" and moves cursor between
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString("{}".to_string()),
            EditCommand::MoveLeft { select: false },
        ]));

        // State: `{|}` - cursor between braces
        assert_eq!(state.buffer, "{}");
        assert_eq!(state.cursor_pos, 1);
        assert_eq!(state.buffer_len, 2);
        assert!(!state.cursor_at_end());
        assert!(!state.uncertain);

        // Check that char after cursor is `}`
        assert_eq!(state.char_after_cursor(), Some('}'));

        // The condition for `(` auto-match should pass
        // (cursor is before closing char `}`)
        assert!(paren_rule.condition.check(&state));

        // Simulate typing `(` with auto-match
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString("()".to_string()),
            EditCommand::MoveLeft { select: false },
        ]));

        // Result: `{(|)}`
        assert_eq!(state.buffer, "{()}");
        assert_eq!(state.cursor_pos, 2); // Between the parens
    }

    /// Test that ReedlineEvent::Left is handled in UntilFound
    ///
    /// Note: In actual reedline, Left arrow binding is:
    /// `UntilFound([MenuLeft, Left])` - NO HistoryHintComplete
    /// (HistoryHintComplete is only in Right arrow binding)
    #[test]
    fn test_until_found_left_updates_cursor() {
        let mut state = EditorState::new();

        // Simulate typing "abc"
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('c')]));
        assert_eq!(state.cursor_pos, 3);
        assert!(state.cursor_at_end());

        // Simulate left arrow key (UntilFound with Left)
        // This matches actual reedline binding: UntilFound([MenuLeft, Left])
        state.update_from_event(&ReedlineEvent::UntilFound(vec![
            ReedlineEvent::MenuLeft,
            ReedlineEvent::Left,
        ]));

        // After Left, cursor should be at position 2
        assert_eq!(state.cursor_pos, 2);
        assert!(!state.cursor_at_end());
    }

    /// Test direct ReedlineEvent::Right handling
    #[test]
    fn test_right_event_updates_cursor() {
        let mut state = EditorState::new();
        state.buffer_len = 5;
        state.cursor_pos = 2;

        state.update_from_event(&ReedlineEvent::Right);
        assert_eq!(state.cursor_pos, 3);

        // At end, should not go past
        state.cursor_pos = 5;
        state.update_from_event(&ReedlineEvent::Right);
        assert_eq!(state.cursor_pos, 5);
    }

    /// Test direct ReedlineEvent::Left handling
    #[test]
    fn test_left_event_updates_cursor() {
        let mut state = EditorState::new();
        state.buffer_len = 5;
        state.cursor_pos = 2;

        state.update_from_event(&ReedlineEvent::Left);
        assert_eq!(state.cursor_pos, 1);

        // At beginning, should not go negative
        state.cursor_pos = 0;
        state.update_from_event(&ReedlineEvent::Left);
        assert_eq!(state.cursor_pos, 0);
    }

    // ===== Tests for buffer content tracking =====

    #[test]
    fn test_char_before_cursor() {
        let mut state = EditorState::new();
        state.buffer = "abc".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 2;

        assert_eq!(state.char_before_cursor(), Some('b'));

        // At beginning, should return None
        state.cursor_pos = 0;
        assert_eq!(state.char_before_cursor(), None);

        // At end
        state.cursor_pos = 3;
        assert_eq!(state.char_before_cursor(), Some('c'));
    }

    #[test]
    fn test_char_after_cursor() {
        let mut state = EditorState::new();
        state.buffer = "abc".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 1;

        assert_eq!(state.char_after_cursor(), Some('b'));

        // At end, should return None
        state.cursor_pos = 3;
        assert_eq!(state.char_after_cursor(), None);

        // At beginning
        state.cursor_pos = 0;
        assert_eq!(state.char_after_cursor(), Some('a'));
    }

    #[test]
    fn test_char_methods_uncertain() {
        let mut state = EditorState::new();
        state.buffer = "abc".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 1;
        state.uncertain = true;

        // When uncertain, both should return None
        assert_eq!(state.char_before_cursor(), None);
        assert_eq!(state.char_after_cursor(), None);
    }

    #[test]
    fn test_is_inside_empty_pair_parens() {
        let mut state = EditorState::new();
        state.buffer = "()".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1; // Between ( and )

        assert!(state.is_inside_empty_pair());
    }

    #[test]
    fn test_is_inside_empty_pair_brackets() {
        let mut state = EditorState::new();
        state.buffer = "[]".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1;

        assert!(state.is_inside_empty_pair());
    }

    #[test]
    fn test_is_inside_empty_pair_braces() {
        let mut state = EditorState::new();
        state.buffer = "{}".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1;

        assert!(state.is_inside_empty_pair());
    }

    #[test]
    fn test_is_inside_empty_pair_quotes() {
        let mut state = EditorState::new();

        // Double quotes
        state.buffer = r#""""#.to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1;
        assert!(state.is_inside_empty_pair());

        // Single quotes
        state.buffer = "''".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1;
        assert!(state.is_inside_empty_pair());

        // Backticks
        state.buffer = "``".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1;
        assert!(state.is_inside_empty_pair());
    }

    #[test]
    fn test_is_inside_empty_pair_not_empty() {
        let mut state = EditorState::new();
        state.buffer = "(x)".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 1; // Before 'x'

        // Not an empty pair - there's content inside
        assert!(!state.is_inside_empty_pair());
    }

    #[test]
    fn test_is_inside_empty_pair_mismatched() {
        let mut state = EditorState::new();
        state.buffer = "(]".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1;

        // Mismatched pair
        assert!(!state.is_inside_empty_pair());
    }

    #[test]
    fn test_is_inside_empty_pair_uncertain() {
        let mut state = EditorState::new();
        state.buffer = "()".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1;
        state.uncertain = true;

        // When uncertain, should return false for safety
        assert!(!state.is_inside_empty_pair());
    }

    #[test]
    fn test_is_inside_empty_pair_edge_positions() {
        let mut state = EditorState::new();
        state.buffer = "()".to_string();
        state.buffer_len = 2;

        // At beginning (before '(')
        state.cursor_pos = 0;
        assert!(!state.is_inside_empty_pair());

        // At end (after ')')
        state.cursor_pos = 2;
        assert!(!state.is_inside_empty_pair());
    }

    #[test]
    fn test_reset_clears_buffer() {
        let mut state = EditorState::new();
        state.buffer = "test".to_string();
        state.buffer_len = 4;
        state.cursor_pos = 2;
        state.uncertain = true;

        state.reset();

        assert!(state.buffer.is_empty());
        assert_eq!(state.buffer_len, 0);
        assert_eq!(state.cursor_pos, 0);
        assert!(!state.uncertain);
    }

    // ===== Tests for buffer content updates via events =====

    #[test]
    fn test_insert_char_updates_buffer() {
        let mut state = EditorState::new();

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('a')]));
        assert_eq!(state.buffer, "a");
        assert_eq!(state.cursor_pos, 1);
        assert_eq!(state.buffer_len, 1);

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
        assert_eq!(state.buffer, "ab");
        assert_eq!(state.cursor_pos, 2);
        assert_eq!(state.buffer_len, 2);
    }

    #[test]
    fn test_insert_string_updates_buffer() {
        let mut state = EditorState::new();

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertString(
            "hello".to_string(),
        )]));
        assert_eq!(state.buffer, "hello");
        assert_eq!(state.cursor_pos, 5);
        assert_eq!(state.buffer_len, 5);
    }

    #[test]
    fn test_insert_char_in_middle() {
        let mut state = EditorState::new();
        state.buffer = "ac".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1; // After 'a'

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('b')]));
        assert_eq!(state.buffer, "abc");
        assert_eq!(state.cursor_pos, 2);
        assert_eq!(state.buffer_len, 3);
    }

    #[test]
    fn test_backspace_updates_buffer() {
        let mut state = EditorState::new();
        state.buffer = "abc".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 3;

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));
        assert_eq!(state.buffer, "ab");
        assert_eq!(state.cursor_pos, 2);
        assert_eq!(state.buffer_len, 2);
    }

    #[test]
    fn test_backspace_in_middle() {
        let mut state = EditorState::new();
        state.buffer = "abc".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 2; // After 'b'

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));
        assert_eq!(state.buffer, "ac");
        assert_eq!(state.cursor_pos, 1);
        assert_eq!(state.buffer_len, 2);
    }

    #[test]
    fn test_delete_updates_buffer() {
        let mut state = EditorState::new();
        state.buffer = "abc".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 1; // After 'a'

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Delete]));
        assert_eq!(state.buffer, "ac");
        assert_eq!(state.cursor_pos, 1);
        assert_eq!(state.buffer_len, 2);
    }

    #[test]
    fn test_cut_from_start_updates_buffer() {
        let mut state = EditorState::new();
        state.buffer = "hello world".to_string();
        state.buffer_len = 11;
        state.cursor_pos = 6; // After "hello "

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::CutFromStart]));
        assert_eq!(state.buffer, "world");
        assert_eq!(state.cursor_pos, 0);
        assert_eq!(state.buffer_len, 5);
    }

    #[test]
    fn test_cut_to_end_updates_buffer() {
        let mut state = EditorState::new();
        state.buffer = "hello world".to_string();
        state.buffer_len = 11;
        state.cursor_pos = 5; // After "hello"

        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::CutToEnd]));
        assert_eq!(state.buffer, "hello");
        assert_eq!(state.cursor_pos, 5);
        assert_eq!(state.buffer_len, 5);
    }

    #[test]
    fn test_history_navigation_sets_uncertain() {
        let mut state = EditorState::new();
        state.buffer = "test".to_string();
        state.buffer_len = 4;
        state.cursor_pos = 4;
        assert!(!state.uncertain);

        state.update_from_event(&ReedlineEvent::Up);
        assert!(state.uncertain);
    }

    #[test]
    fn test_unicode_insert_and_delete() {
        let mut state = EditorState::new();

        // Insert Unicode characters
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('日')]));
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('本')]));
        assert_eq!(state.buffer, "日本");
        assert_eq!(state.cursor_pos, 2);
        assert_eq!(state.buffer_len, 2);

        // Backspace removes one character
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));
        assert_eq!(state.buffer, "日");
        assert_eq!(state.cursor_pos, 1);
        assert_eq!(state.buffer_len, 1);
    }

    #[test]
    fn test_auto_pair_simulation() {
        let mut state = EditorState::new();

        // Simulate typing '(' which triggers auto-match: InsertString("()") + MoveLeft
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString("()".to_string()),
            EditCommand::MoveLeft { select: false },
        ]));
        assert_eq!(state.buffer, "()");
        assert_eq!(state.cursor_pos, 1);
        assert_eq!(state.buffer_len, 2);

        // Now cursor is inside empty pair
        assert!(state.is_inside_empty_pair());

        // Backspace should delete '('
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));
        assert_eq!(state.buffer, ")");
        assert_eq!(state.cursor_pos, 0);
        assert_eq!(state.buffer_len, 1);
    }

    // ===== Tests for bracket delete rules =====

    #[test]
    fn test_bracket_delete_rules_created() {
        let rules = create_bracket_delete_rules();
        assert_eq!(rules.len(), 1);
    }

    #[test]
    fn test_bracket_delete_rule_matches_backspace() {
        let rules = create_bracket_delete_rules();
        let rule = &rules[0];

        // Should match a single Backspace command
        let backspace_event = ReedlineEvent::Edit(vec![EditCommand::Backspace]);
        assert!((rule.match_event)(&backspace_event));

        // Should not match other events
        let delete_event = ReedlineEvent::Edit(vec![EditCommand::Delete]);
        assert!(!(rule.match_event)(&delete_event));

        // Should not match multiple commands
        let multiple_event = ReedlineEvent::Edit(vec![
            EditCommand::Backspace,
            EditCommand::Delete,
        ]);
        assert!(!(rule.match_event)(&multiple_event));
    }

    #[test]
    fn test_bracket_delete_condition_not_inside_pair() {
        let rules = create_bracket_delete_rules();
        let rule = &rules[0];

        // Not inside pair - condition should return true (keep original backspace)
        let mut state = EditorState::new();
        state.buffer = "abc".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 2;
        assert!(rule.condition.check(&state));
    }

    #[test]
    fn test_bracket_delete_condition_inside_pair() {
        let rules = create_bracket_delete_rules();
        let rule = &rules[0];

        // Inside empty pair - condition should return false (use fallback)
        let mut state = EditorState::new();
        state.buffer = "()".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1;
        assert!(!rule.condition.check(&state));
    }

    #[test]
    fn test_bracket_delete_fallback_event() {
        let rules = create_bracket_delete_rules();
        let rule = &rules[0];

        // Fallback should be Backspace + Delete
        match &rule.fallback_event {
            ReedlineEvent::Edit(cmds) => {
                assert_eq!(cmds.len(), 2);
                assert!(matches!(&cmds[0], EditCommand::Backspace));
                assert!(matches!(&cmds[1], EditCommand::Delete));
            }
            _ => panic!("Expected Edit event"),
        }
    }

    #[test]
    fn test_bracket_delete_full_flow() {
        let mut state = EditorState::new();

        // Type '(' with auto-match: inserts "()" and moves cursor between
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString("()".to_string()),
            EditCommand::MoveLeft { select: false },
        ]));
        assert_eq!(state.buffer, "()");
        assert_eq!(state.cursor_pos, 1);
        assert!(state.is_inside_empty_pair());

        // Simulate the bracket delete rule's fallback event: Backspace + Delete
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::Backspace,
            EditCommand::Delete,
        ]));

        // Both brackets should be deleted
        assert_eq!(state.buffer, "");
        assert_eq!(state.cursor_pos, 0);
        assert_eq!(state.buffer_len, 0);
    }

    #[test]
    fn test_bracket_delete_all_pair_types() {
        let pairs = [("()", 1), ("[]", 1), ("{}", 1), (r#""""#, 1), ("''", 1), ("``", 1)];

        for (pair, cursor_pos) in pairs {
            let mut state = EditorState::new();
            state.buffer = pair.to_string();
            state.buffer_len = 2;
            state.cursor_pos = cursor_pos;

            assert!(
                state.is_inside_empty_pair(),
                "Expected to be inside empty pair for: {}",
                pair
            );

            // Apply the fallback event
            state.update_from_event(&ReedlineEvent::Edit(vec![
                EditCommand::Backspace,
                EditCommand::Delete,
            ]));

            assert_eq!(state.buffer, "", "Buffer should be empty after deleting {}", pair);
        }
    }

    #[test]
    fn test_bracket_delete_uncertain_state() {
        let rules = create_bracket_delete_rules();
        let rule = &rules[0];

        // When state is uncertain, condition should return true (keep original backspace)
        let mut state = EditorState::new();
        state.buffer = "()".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1;
        state.uncertain = true;

        // is_inside_empty_pair returns false when uncertain
        assert!(!state.is_inside_empty_pair());
        // So NotInsideEmptyPair returns true
        assert!(rule.condition.check(&state));
    }

    // ===== Tests for CursorAtEndOrBeforeClosing =====

    #[test]
    fn test_cursor_at_end_or_before_closing_at_end() {
        let condition = CursorAtEndOrBeforeClosing;

        // Empty buffer: cursor at end
        let mut state = EditorState::new();
        assert!(condition.check(&state));

        // Buffer with content, cursor at end
        state.buffer = "foo".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 3;
        assert!(condition.check(&state));
    }

    #[test]
    fn test_cursor_at_end_or_before_closing_before_closing_chars() {
        let condition = CursorAtEndOrBeforeClosing;

        // Test each closing character
        let closing_chars = [')', ']', '}', '"', '\'', '`'];

        for closing_char in closing_chars {
            let mut state = EditorState::new();
            state.buffer = format!("x{}", closing_char);
            state.buffer_len = 2;
            state.cursor_pos = 1; // Between 'x' and closing char

            assert!(
                condition.check(&state),
                "Expected condition to pass when cursor is before '{}'",
                closing_char
            );
        }
    }

    #[test]
    fn test_cursor_at_end_or_before_closing_before_regular_char() {
        let condition = CursorAtEndOrBeforeClosing;

        // Cursor before a regular character (not closing)
        let mut state = EditorState::new();
        state.buffer = "abc".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 1; // Between 'a' and 'b'

        assert!(!condition.check(&state));
    }

    #[test]
    fn test_cursor_at_end_or_before_closing_uncertain() {
        let condition = CursorAtEndOrBeforeClosing;

        // When uncertain and not at end, should return false
        let mut state = EditorState::new();
        state.buffer = "()".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1;
        state.uncertain = true;

        // char_after_cursor returns None when uncertain
        assert!(!condition.check(&state));
    }

    // ===== Tests for nested bracket/quote scenarios =====

    /// Test: `(│)` + `"` → `("│")`
    #[test]
    fn test_auto_match_quote_inside_parens() {
        let condition = CursorAtEndOrBeforeClosing;

        // State: `(│)` - cursor between parens
        let mut state = EditorState::new();
        state.buffer = "()".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1; // Before ')'

        // Condition should pass (cursor is before ')')
        assert!(condition.check(&state));

        // Simulate typing `"` with auto-match: InsertString("\"\"") + MoveLeft
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString(r#""""#.to_string()),
            EditCommand::MoveLeft { select: false },
        ]));

        // Result: `("│")`
        assert_eq!(state.buffer, r#"("")"#);
        assert_eq!(state.cursor_pos, 2); // Between the quotes
    }

    /// Test: `"│"` + `(` → `"(│)"`
    #[test]
    fn test_auto_match_parens_inside_quotes() {
        let condition = CursorAtEndOrBeforeClosing;

        // State: `"│"` - cursor between quotes
        let mut state = EditorState::new();
        state.buffer = r#""""#.to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1; // Before closing quote

        // Condition should pass (cursor is before '"')
        assert!(condition.check(&state));

        // Simulate typing `(` with auto-match: InsertString("()") + MoveLeft
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString("()".to_string()),
            EditCommand::MoveLeft { select: false },
        ]));

        // Result: `"(│)"`
        assert_eq!(state.buffer, r#""()""#);
        assert_eq!(state.cursor_pos, 2); // Between the parens
    }

    /// Test: `[│]` + `{` → `[{│}]`
    #[test]
    fn test_auto_match_braces_inside_brackets() {
        let condition = CursorAtEndOrBeforeClosing;

        // State: `[│]` - cursor between brackets
        let mut state = EditorState::new();
        state.buffer = "[]".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1; // Before ']'

        // Condition should pass (cursor is before ']')
        assert!(condition.check(&state));

        // Simulate typing `{` with auto-match
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString("{}".to_string()),
            EditCommand::MoveLeft { select: false },
        ]));

        // Result: `[{│}]`
        assert_eq!(state.buffer, "[{}]");
        assert_eq!(state.cursor_pos, 2);
    }

    /// Test: `r"│"` + `(` → `r"(│)"`
    #[test]
    fn test_auto_match_parens_inside_raw_string() {
        let condition = CursorAtEndOrBeforeClosing;

        // State: `r"│"` - cursor between quotes of raw string
        let mut state = EditorState::new();
        state.buffer = r#"r"""#.to_string();
        state.buffer_len = 3;
        state.cursor_pos = 2; // Before closing quote

        // Condition should pass (cursor is before '"')
        assert!(condition.check(&state));

        // Simulate typing `(` with auto-match
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString("()".to_string()),
            EditCommand::MoveLeft { select: false },
        ]));

        // Result: `r"(│)"`
        assert_eq!(state.buffer, r#"r"()""#);
        assert_eq!(state.cursor_pos, 3);
    }

    /// Test: `foo│` + `(` → `foo(│)` (cursor at end, keep working)
    #[test]
    fn test_auto_match_at_end_still_works() {
        let condition = CursorAtEndOrBeforeClosing;

        // State: `foo│` - cursor at end
        let mut state = EditorState::new();
        state.buffer = "foo".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 3; // At end

        // Condition should pass (cursor at end)
        assert!(condition.check(&state));

        // Simulate typing `(` with auto-match
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString("()".to_string()),
            EditCommand::MoveLeft { select: false },
        ]));

        // Result: `foo(│)`
        assert_eq!(state.buffer, "foo()");
        assert_eq!(state.cursor_pos, 4);
    }

    /// Test: `foo│bar` + `(` → `foo(│bar` (no auto-match in middle)
    #[test]
    fn test_auto_match_blocked_in_middle() {
        let condition = CursorAtEndOrBeforeClosing;

        // State: `foo│bar` - cursor in middle
        let mut state = EditorState::new();
        state.buffer = "foobar".to_string();
        state.buffer_len = 6;
        state.cursor_pos = 3; // Before 'b'

        // Condition should fail (cursor before 'b' which is not a closing char)
        assert!(!condition.check(&state));
    }

    /// Test: deeply nested brackets work correctly
    #[test]
    fn test_auto_match_deeply_nested() {
        let condition = CursorAtEndOrBeforeClosing;

        // Start with `(│)`, add multiple levels of nesting
        let mut state = EditorState::new();
        state.buffer = "()".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1;

        // Add `[` inside parens: `([│])`
        assert!(condition.check(&state));
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString("[]".to_string()),
            EditCommand::MoveLeft { select: false },
        ]));
        assert_eq!(state.buffer, "([])");
        assert_eq!(state.cursor_pos, 2);

        // Add `{` inside brackets: `([{│}])`
        assert!(condition.check(&state));
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString("{}".to_string()),
            EditCommand::MoveLeft { select: false },
        ]));
        assert_eq!(state.buffer, "([{}])");
        assert_eq!(state.cursor_pos, 3);

        // Add `"` inside braces: `([{"│"}])`
        assert!(condition.check(&state));
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString(r#""""#.to_string()),
            EditCommand::MoveLeft { select: false },
        ]));
        assert_eq!(state.buffer, r#"([{""}])"#);
        assert_eq!(state.cursor_pos, 4);
    }

    #[test]
    fn test_auto_match_rules_use_new_condition() {
        let rules = create_auto_match_rules();
        let paren_rule = &rules[0]; // '(' rule

        // Test that condition passes when cursor is before closing char
        let mut state = EditorState::new();
        state.buffer = "()".to_string();
        state.buffer_len = 2;
        state.cursor_pos = 1; // Before ')'

        // This should pass with the new CursorAtEndOrBeforeClosing condition
        assert!(paren_rule.condition.check(&state));

        // Test that condition fails when cursor is before regular char
        state.buffer = "ab".to_string();
        state.cursor_pos = 1; // Before 'b'

        assert!(!paren_rule.condition.check(&state));
    }

    // ===== Tests for cursor_in_quote and quote auto-match =====

    #[test]
    fn test_cursor_in_quote_empty() {
        let state = EditorState::new();
        assert!(!state.cursor_in_quote('"'));
        assert!(!state.cursor_in_quote('\''));
    }

    #[test]
    fn test_cursor_in_quote_unclosed() {
        let mut state = EditorState::new();

        // `"foo|` - inside unclosed double quote
        state.buffer = r#""foo"#.to_string();
        state.buffer_len = 4;
        state.cursor_pos = 4;
        assert!(state.cursor_in_quote('"'));
        assert!(!state.cursor_in_quote('\'')); // Not inside single quote

        // `'foo|` - inside unclosed single quote
        state.buffer = "'foo".to_string();
        state.buffer_len = 4;
        state.cursor_pos = 4;
        assert!(state.cursor_in_quote('\''));
        assert!(!state.cursor_in_quote('"'));
    }

    #[test]
    fn test_cursor_in_quote_closed() {
        let mut state = EditorState::new();

        // `"foo"|` - after closed string
        state.buffer = r#""foo""#.to_string();
        state.buffer_len = 5;
        state.cursor_pos = 5;
        assert!(!state.cursor_in_quote('"'));

        // `"foo" |` - after closed string with space
        state.buffer = r#""foo" "#.to_string();
        state.buffer_len = 6;
        state.cursor_pos = 6;
        assert!(!state.cursor_in_quote('"'));
    }

    #[test]
    fn test_cursor_in_quote_escaped() {
        let mut state = EditorState::new();

        // `"foo\"bar|` - escaped quote, still inside string
        state.buffer = r#""foo\"bar"#.to_string();
        state.buffer_len = 9;
        state.cursor_pos = 9;
        assert!(state.cursor_in_quote('"'));

        // `"foo\"bar"|` - escaped quote, string closed
        state.buffer = r#""foo\"bar""#.to_string();
        state.buffer_len = 10;
        state.cursor_pos = 10;
        assert!(!state.cursor_in_quote('"'));
    }

    #[test]
    fn test_cursor_in_quote_uncertain() {
        let mut state = EditorState::new();
        state.buffer = r#""foo"#.to_string();
        state.buffer_len = 4;
        state.cursor_pos = 4;
        state.uncertain = true;

        // When uncertain, should return false for safety
        assert!(!state.cursor_in_quote('"'));
    }

    #[test]
    fn test_quote_auto_match_condition_not_in_string() {
        let condition = CursorAtEndOrBeforeClosingAndNotInQuote::new('"');

        // `foo|` - not in string, at end
        let mut state = EditorState::new();
        state.buffer = "foo".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 3;

        // Should allow auto-match
        assert!(condition.check(&state));
    }

    #[test]
    fn test_quote_auto_match_condition_in_unclosed_string() {
        let condition = CursorAtEndOrBeforeClosingAndNotInQuote::new('"');

        // `"foo|` - inside unclosed string
        let mut state = EditorState::new();
        state.buffer = r#""foo"#.to_string();
        state.buffer_len = 4;
        state.cursor_pos = 4;

        // Should NOT allow auto-match (we want to just close the string)
        assert!(!condition.check(&state));
    }

    #[test]
    fn test_quote_auto_match_condition_after_closed_string() {
        let condition = CursorAtEndOrBeforeClosingAndNotInQuote::new('"');

        // `"foo" |` - after closed string
        let mut state = EditorState::new();
        state.buffer = r#""foo" "#.to_string();
        state.buffer_len = 6;
        state.cursor_pos = 6;

        // Should allow auto-match (starting a new string)
        assert!(condition.check(&state));
    }

    /// Test: `"foo|` + `"` → `"foo"|` (close the string, don't insert pair)
    #[test]
    fn test_quote_auto_match_closes_string() {
        let rules = create_auto_match_rules();
        let quote_rule = &rules[3]; // '"' rule

        // State: `"foo|` - inside unclosed string
        let mut state = EditorState::new();
        state.buffer = r#""foo"#.to_string();
        state.buffer_len = 4;
        state.cursor_pos = 4;

        // Condition should fail (we're inside an unclosed string)
        assert!(!quote_rule.condition.check(&state));

        // The fallback should be InsertChar('"'), not InsertString("\"\"")
        match &quote_rule.fallback_event {
            ReedlineEvent::Edit(cmds) => {
                assert_eq!(cmds.len(), 1);
                assert!(matches!(&cmds[0], EditCommand::InsertChar('"')));
            }
            _ => panic!("Expected Edit event"),
        }
    }

    /// Test: `foo|` + `"` → `foo"|"` (not in string, auto-match works)
    #[test]
    fn test_quote_auto_match_works_outside_string() {
        let rules = create_auto_match_rules();
        let quote_rule = &rules[3]; // '"' rule

        // State: `foo|` - not inside any string
        let mut state = EditorState::new();
        state.buffer = "foo".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 3;

        // Condition should pass (not inside unclosed string, cursor at end)
        assert!(quote_rule.condition.check(&state));
    }

    /// Test: single quotes work the same way
    #[test]
    fn test_single_quote_auto_match_in_string() {
        let rules = create_auto_match_rules();
        let quote_rule = &rules[4]; // '\'' rule

        // State: `'foo|` - inside unclosed single-quoted string
        let mut state = EditorState::new();
        state.buffer = "'foo".to_string();
        state.buffer_len = 4;
        state.cursor_pos = 4;

        // Condition should fail
        assert!(!quote_rule.condition.check(&state));
    }

    /// Test: backticks work the same way
    #[test]
    fn test_backtick_auto_match_in_string() {
        let rules = create_auto_match_rules();
        let quote_rule = &rules[5]; // '`' rule

        // State: `` `foo| `` - inside unclosed backtick
        let mut state = EditorState::new();
        state.buffer = "`foo".to_string();
        state.buffer_len = 4;
        state.cursor_pos = 4;

        // Condition should fail
        assert!(!quote_rule.condition.check(&state));
    }

    // ===== Tests for backspace after newline inside brackets =====

    /// Test: backspace after newline inside brackets should NOT delete the closing bracket.
    ///
    /// Bug scenario:
    /// 1. Type '(' -> auto-match gives "()" with cursor between
    /// 2. Press Enter -> buffer becomes "(\n)" with cursor after newline
    /// 3. Press Backspace -> should delete only newline, NOT the closing bracket
    ///
    /// The bracket delete rule should NOT trigger because char_before_cursor is '\n', not '('.
    #[test]
    fn test_backspace_after_newline_not_inside_empty_pair() {
        let mut state = EditorState::new();

        // Simulate typing '(' with auto-match
        state.update_from_event(&ReedlineEvent::Edit(vec![
            EditCommand::InsertString("()".to_string()),
            EditCommand::MoveLeft { select: false },
        ]));
        assert_eq!(state.buffer, "()");
        assert_eq!(state.cursor_pos, 1);
        assert!(state.is_inside_empty_pair());

        // Simulate pressing Enter (InsertNewline)
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertNewline]));
        assert_eq!(state.buffer, "(\n)");
        assert_eq!(state.cursor_pos, 2);

        // After newline, we should NOT be inside an empty pair
        // because char_before_cursor is '\n', not '('
        assert!(!state.is_inside_empty_pair());

        // Verify the characters
        assert_eq!(state.char_before_cursor(), Some('\n'));
        assert_eq!(state.char_after_cursor(), Some(')'));
    }

    /// Test: bracket delete rule condition after newline inside brackets.
    ///
    /// NotInsideEmptyPair should return true (keep original Backspace)
    /// because we're not inside an empty pair after the newline.
    #[test]
    fn test_bracket_delete_condition_after_newline() {
        let rules = create_bracket_delete_rules();
        let rule = &rules[0];

        let mut state = EditorState::new();
        state.buffer = "(\n)".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 2; // After '\n', before ')'

        // is_inside_empty_pair should be false
        assert!(!state.is_inside_empty_pair());

        // NotInsideEmptyPair should return true (keep original Backspace)
        assert!(rule.condition.check(&state));
    }

    /// Test: backspace after newline should only delete the newline.
    #[test]
    fn test_backspace_after_newline_deletes_only_newline() {
        let mut state = EditorState::new();
        state.buffer = "(\n)".to_string();
        state.buffer_len = 3;
        state.cursor_pos = 2;

        // Apply single Backspace (not Backspace + Delete)
        state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));

        // Should only delete the newline
        assert_eq!(state.buffer, "()");
        assert_eq!(state.cursor_pos, 1);
        assert_eq!(state.buffer_len, 2);
    }

    // ===== Tests for tree-sitter word navigation =====

    /// Test: create_move_event generates correct left movement
    #[test]
    fn test_create_move_event_left() {
        use super::ConditionalEditMode;
        use reedline::Emacs;

        // Moving from position 5 to position 2 should generate 3 MoveLeft commands
        let event = ConditionalEditMode::<Emacs>::create_move_event(5, 2, false);

        match event {
            ReedlineEvent::Edit(commands) => {
                assert_eq!(commands.len(), 3);
                for cmd in commands {
                    assert!(matches!(cmd, EditCommand::MoveLeft { select: false }));
                }
            }
            _ => panic!("Expected Edit event with MoveLeft commands"),
        }
    }

    /// Test: create_move_event generates correct right movement
    #[test]
    fn test_create_move_event_right() {
        use super::ConditionalEditMode;
        use reedline::Emacs;

        // Moving from position 2 to position 5 should generate 3 MoveRight commands
        let event = ConditionalEditMode::<Emacs>::create_move_event(2, 5, false);

        match event {
            ReedlineEvent::Edit(commands) => {
                assert_eq!(commands.len(), 3);
                for cmd in commands {
                    assert!(matches!(cmd, EditCommand::MoveRight { select: false }));
                }
            }
            _ => panic!("Expected Edit event with MoveRight commands"),
        }
    }

    /// Test: create_move_event with select=true
    #[test]
    fn test_create_move_event_with_selection() {
        use super::ConditionalEditMode;
        use reedline::Emacs;

        let event = ConditionalEditMode::<Emacs>::create_move_event(2, 5, true);

        match event {
            ReedlineEvent::Edit(commands) => {
                assert_eq!(commands.len(), 3);
                for cmd in commands {
                    assert!(matches!(cmd, EditCommand::MoveRight { select: true }));
                }
            }
            _ => panic!("Expected Edit event with MoveRight commands"),
        }
    }

    /// Test: create_move_event returns None when positions are equal
    #[test]
    fn test_create_move_event_same_position() {
        use super::ConditionalEditMode;
        use reedline::Emacs;

        let event = ConditionalEditMode::<Emacs>::create_move_event(5, 5, false);
        assert!(matches!(event, ReedlineEvent::None));
    }

    /// Test: handle_tree_sitter_word_nav transforms MoveWordRight for pipe operator
    #[test]
    fn test_tree_sitter_word_nav_pipe_operator() {
        use super::ConditionalEditMode;
        use reedline::Emacs;

        let state_ref = new_editor_state_ref();

        // Set up state: "x |> filter()"
        //                  ^-- cursor at position 2 (before |>)
        {
            let mut state = state_ref.lock().unwrap();
            state.buffer = "x |> filter()".to_string();
            state.buffer_len = 13;
            state.cursor_pos = 2;
            state.uncertain = false;
        }

        let conditional = ConditionalEditMode::new(Emacs::default(), state_ref.clone())
            .with_tree_sitter_word_nav(true);

        // Test MoveWordRight - should jump over "|>" as a single token
        let event = ReedlineEvent::Edit(vec![EditCommand::MoveWordRight { select: false }]);
        let result = conditional.handle_tree_sitter_word_nav(&event);

        assert!(result.is_some());
        let result_event = result.unwrap();

        // Should generate MoveRight commands to move from position 2 to 4 (after "|>")
        match result_event {
            ReedlineEvent::Edit(commands) => {
                assert_eq!(commands.len(), 2); // Move 2 positions right
                for cmd in commands {
                    assert!(matches!(cmd, EditCommand::MoveRight { select: false }));
                }
            }
            _ => panic!("Expected Edit event"),
        }
    }

    /// Test: handle_tree_sitter_word_nav transforms MoveWordLeft for assignment
    #[test]
    fn test_tree_sitter_word_nav_assignment_left() {
        use super::ConditionalEditMode;
        use reedline::Emacs;

        let state_ref = new_editor_state_ref();

        // Set up state: "x <- 42"
        //                   ^-- cursor at position 4 (after "<-")
        {
            let mut state = state_ref.lock().unwrap();
            state.buffer = "x <- 42".to_string();
            state.buffer_len = 7;
            state.cursor_pos = 4;
            state.uncertain = false;
        }

        let conditional = ConditionalEditMode::new(Emacs::default(), state_ref.clone())
            .with_tree_sitter_word_nav(true);

        // Test MoveWordLeft - should jump to start of "<-"
        let event = ReedlineEvent::Edit(vec![EditCommand::MoveWordLeft { select: false }]);
        let result = conditional.handle_tree_sitter_word_nav(&event);

        assert!(result.is_some());
        let result_event = result.unwrap();

        // Should generate MoveLeft commands to move from position 4 to 2 (start of "<-")
        match result_event {
            ReedlineEvent::Edit(commands) => {
                assert_eq!(commands.len(), 2); // Move 2 positions left
                for cmd in commands {
                    assert!(matches!(cmd, EditCommand::MoveLeft { select: false }));
                }
            }
            _ => panic!("Expected Edit event"),
        }
    }

    /// Test: handle_tree_sitter_word_nav is disabled when tree_sitter_word_nav=false
    #[test]
    fn test_tree_sitter_word_nav_disabled() {
        use super::ConditionalEditMode;
        use reedline::Emacs;

        let state_ref = new_editor_state_ref();

        {
            let mut state = state_ref.lock().unwrap();
            state.buffer = "x |> filter()".to_string();
            state.buffer_len = 13;
            state.cursor_pos = 2;
            state.uncertain = false;
        }

        // tree-sitter word nav is disabled by default
        let conditional = ConditionalEditMode::new(Emacs::default(), state_ref.clone());

        let event = ReedlineEvent::Edit(vec![EditCommand::MoveWordRight { select: false }]);
        let result = conditional.handle_tree_sitter_word_nav(&event);

        // Should return None when disabled
        assert!(result.is_none());
    }

    /// Test: handle_tree_sitter_word_nav returns None when state is uncertain
    #[test]
    fn test_tree_sitter_word_nav_uncertain_state() {
        use super::ConditionalEditMode;
        use reedline::Emacs;

        let state_ref = new_editor_state_ref();

        {
            let mut state = state_ref.lock().unwrap();
            state.buffer = "x |> filter()".to_string();
            state.buffer_len = 13;
            state.cursor_pos = 2;
            state.uncertain = true; // State is uncertain
        }

        let conditional = ConditionalEditMode::new(Emacs::default(), state_ref.clone())
            .with_tree_sitter_word_nav(true);

        let event = ReedlineEvent::Edit(vec![EditCommand::MoveWordRight { select: false }]);
        let result = conditional.handle_tree_sitter_word_nav(&event);

        // Should return None when state is uncertain (falls back to default behavior)
        assert!(result.is_none());
    }

    /// Test: handle_tree_sitter_word_nav handles UntilFound with word movement
    #[test]
    fn test_tree_sitter_word_nav_until_found() {
        use super::ConditionalEditMode;
        use reedline::Emacs;

        let state_ref = new_editor_state_ref();

        {
            let mut state = state_ref.lock().unwrap();
            state.buffer = "x |> filter()".to_string();
            state.buffer_len = 13;
            state.cursor_pos = 2;
            state.uncertain = false;
        }

        let conditional = ConditionalEditMode::new(Emacs::default(), state_ref.clone())
            .with_tree_sitter_word_nav(true);

        // Simulate Ctrl+Right binding: UntilFound with HistoryHintWordComplete and MoveWordRight
        let event = ReedlineEvent::UntilFound(vec![
            ReedlineEvent::HistoryHintWordComplete,
            ReedlineEvent::Edit(vec![EditCommand::MoveWordRight { select: false }]),
        ]);
        let result = conditional.handle_tree_sitter_word_nav(&event);

        assert!(result.is_some());
        let result_event = result.unwrap();

        // Should be UntilFound with replaced MoveWordRight
        match result_event {
            ReedlineEvent::UntilFound(events) => {
                assert_eq!(events.len(), 2);
                // First event should still be HistoryHintWordComplete
                assert!(matches!(events[0], ReedlineEvent::HistoryHintWordComplete));
                // Second event should be transformed to MoveRight commands
                match &events[1] {
                    ReedlineEvent::Edit(commands) => {
                        assert_eq!(commands.len(), 2);
                    }
                    _ => panic!("Expected Edit event"),
                }
            }
            _ => panic!("Expected UntilFound event"),
        }
    }
}
