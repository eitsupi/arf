//! Conditional keybinding rules and the edit mode wrapper that applies them.

use super::state::{EditorState, EditorStateRef};
use reedline::{EditCommand, EditMode, PromptEditMode, ReedlineEvent};

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
    pub(super) fn handle_tree_sitter_word_nav(
        &self,
        event: &ReedlineEvent,
    ) -> Option<ReedlineEvent> {
        use crate::editor::word_nav::{token_left_position, token_right_position};

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
    pub(super) fn create_move_event(current: usize, target: usize, select: bool) -> ReedlineEvent {
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
