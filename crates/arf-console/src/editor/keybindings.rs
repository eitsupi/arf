//! Keyboard shortcut configuration.

use crate::editor::mode::{
    ConditionalEditMode, ConditionalRule, CursorAtBegin, EditorStateRef,
    create_auto_match_rules, create_bracket_delete_rules, create_skip_over_rules,
};
use crokey::KeyCombination;
use reedline::{EditCommand, EditMode, KeyCode, KeyModifiers, Keybindings, ReedlineEvent};
use std::collections::HashMap;

/// Add common keybindings to an existing keybinding set.
///
/// Enter submits the input, Shift+Enter inserts a newline.
pub fn add_common_keybindings(keybindings: &mut Keybindings) {
    // Tab for completion
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion_menu".to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );

    // ':' triggers completion menu for meta commands
    // This provides immediate feedback when entering meta command mode
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Char(':'),
        ReedlineEvent::Multiple(vec![
            ReedlineEvent::Edit(vec![EditCommand::InsertChar(':')]),
            ReedlineEvent::Menu("completion_menu".to_string()),
        ]),
    );

    // Ctrl+R for history search menu (shows multiple candidates)
    // First move cursor to end of buffer to ensure history selection replaces
    // the entire buffer. Without this, if cursor is in the middle (e.g., after
    // auto-match inserts a pair like ``), the selection would only replace text
    // up to the cursor, leaving trailing characters.
    keybindings.add_binding(
        KeyModifiers::CONTROL,
        KeyCode::Char('r'),
        ReedlineEvent::Multiple(vec![
            ReedlineEvent::Edit(vec![EditCommand::MoveToEnd { select: false }]),
            ReedlineEvent::UntilFound(vec![
                ReedlineEvent::Menu("history_menu".to_string()),
                ReedlineEvent::MenuPageNext,
            ]),
        ]),
    );

    // Enter submits, Shift+Enter inserts newline
    // Note: Enter submit is already the default behavior in reedline
    keybindings.add_binding(
        KeyModifiers::SHIFT,
        KeyCode::Enter,
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );
}

/// Wrap an EditMode with ConditionalEditMode for context-aware keybindings.
///
/// This applies several conditional rules:
/// - ':' only triggers completion menu when at line start (not in `1:10`)
/// - Auto-match only inserts closing brackets when cursor is at end of buffer
/// - Auto-trigger completion when buffer reaches `completion_min_chars` characters
/// - Tree-sitter based word navigation for Ctrl+Arrow (R token boundaries)
pub fn wrap_edit_mode_with_conditional_rules<E: EditMode + 'static>(
    edit_mode: E,
    state: EditorStateRef,
    auto_match: bool,
    completion_min_chars: Option<usize>,
) -> Box<dyn EditMode> {
    // Rule: when ':' produces InsertChar + Menu, check if cursor is at position 0
    // If not at position 0, replace with just InsertChar(':')
    let colon_rule = ConditionalRule {
        match_event: Box::new(|event| {
            // Match the exact event pattern from add_common_keybindings
            matches!(
                event,
                ReedlineEvent::Multiple(events)
                if events.len() == 2
                    && matches!(&events[0], ReedlineEvent::Edit(cmds) if cmds.len() == 1 && matches!(&cmds[0], EditCommand::InsertChar(':')))
                    && matches!(&events[1], ReedlineEvent::Menu(name) if name == "completion_menu")
            )
        }),
        condition: Box::new(CursorAtBegin),
        fallback_event: ReedlineEvent::Edit(vec![EditCommand::InsertChar(':')]),
    };

    let mut conditional = ConditionalEditMode::new(edit_mode, state)
        .with_rule(colon_rule)
        .with_completion_min_chars(completion_min_chars)
        .with_tree_sitter_word_nav(true);

    // Add smart auto-match rules if enabled
    if auto_match {
        // Bracket delete rules must come first - they handle backspace in empty pairs
        conditional = conditional.with_rules(create_bracket_delete_rules());
        // Skip-over rules must come before auto-match - when cursor is before a closing
        // character, just move right instead of inserting another character
        conditional = conditional.with_rules(create_skip_over_rules());
        // Auto-match rules handle inserting closing brackets
        conditional = conditional.with_rules(create_auto_match_rules());
    }

    Box::new(conditional)
}

/// Add auto-match keybindings for brackets and quotes.
///
/// When typing an opening bracket or quote, automatically inserts the closing
/// counterpart and positions the cursor between them.
pub fn add_auto_match_keybindings(keybindings: &mut Keybindings) {
    // Define pairs: (opening char, pair string)
    let pairs = [
        ('(', "()"),
        ('[', "[]"),
        ('{', "{}"),
        ('"', r#""""#),
        ('\'', "''"),
        ('`', "``"),
    ];

    for (open_char, pair) in pairs {
        let event = ReedlineEvent::Edit(vec![
            EditCommand::InsertString(pair.to_string()),
            EditCommand::MoveLeft { select: false },
        ]);

        // Bind both NONE and SHIFT modifiers for all characters.
        // On Windows, crossterm may include SHIFT in the key event even when
        // Shift is "part of" typing the character (e.g., Shift+9 for '(').
        // Different keyboard layouts have different shift requirements:
        // - US: '(' = Shift+9, '{' = Shift+[
        // - French AZERTY: '(' = 5 (no shift), '{' = AltGr+4
        // By binding both variants, we handle all common layouts.
        keybindings.add_binding(KeyModifiers::NONE, KeyCode::Char(open_char), event.clone());
        keybindings.add_binding(KeyModifiers::SHIFT, KeyCode::Char(open_char), event);
    }
}

/// Add keybindings for inserting text (like assignment and pipe operators).
///
/// Configurable via `editor.key_map` in config using crokey format.
/// Example: "alt-hyphen" = " <- ", "alt-p" = " |> "
pub fn add_key_map_keybindings(
    keybindings: &mut Keybindings,
    key_map: &HashMap<KeyCombination, String>,
) {
    use crossterm::event::KeyEvent;

    for (key_combination, text) in key_map {
        // Convert crokey::KeyCombination to crossterm KeyEvent
        let key_event: KeyEvent = (*key_combination).into();
        keybindings.add_binding(
            key_event.modifiers,
            key_event.code,
            ReedlineEvent::Edit(vec![EditCommand::InsertString(text.clone())]),
        );
    }
}
