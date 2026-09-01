//! Keyboard shortcut configuration.

use crate::editor::mode::{
    BufferKnownEmpty, ConditionalEditMode, ConditionalRule, CursorAtBegin, EditorStateRef,
};
use crokey::KeyCombination;
use reedline::{EditCommand, EditMode, KeyCode, KeyModifiers, Keybindings, ReedlineEvent};
use std::collections::BTreeMap;

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
    // the entire buffer.
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
/// - Auto-trigger completion when buffer reaches `completion_min_chars` characters
/// - Tree-sitter based word navigation for Ctrl+Arrow (R token boundaries)
/// - When `shell_semicolon_shortcut` is true, ';' at empty buffer triggers shell mode
pub fn wrap_edit_mode_with_conditional_rules<E: EditMode + 'static>(
    edit_mode: E,
    state: EditorStateRef,
    completion_min_chars: Option<usize>,
    shell_semicolon_shortcut: bool,
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

    // When the semicolon shortcut is enabled, ';' at an empty buffer inserts
    // ":shell" and submits immediately. When the buffer has content, fall back
    // to inserting a literal semicolon so normal R expressions are unaffected.
    if shell_semicolon_shortcut {
        let semicolon_rule = ConditionalRule {
            match_event: Box::new(|event| {
                matches!(
                    event,
                    ReedlineEvent::Multiple(events)
                    if events.len() == 2
                        && matches!(
                            &events[0],
                            ReedlineEvent::Edit(cmds)
                            if cmds.len() == 1
                                && matches!(&cmds[0], EditCommand::InsertString(s) if s == ":shell")
                        )
                        && matches!(&events[1], ReedlineEvent::Submit)
                )
            }),
            condition: Box::new(BufferKnownEmpty),
            fallback_event: ReedlineEvent::Edit(vec![EditCommand::InsertChar(';')]),
        };
        conditional = conditional.with_rule(semicolon_rule);
    }

    Box::new(conditional)
}

/// Add the `;` → shell-mode keybinding.
///
/// Maps `;` to `Multiple([InsertString(":shell"), Submit])` so that pressing
/// `;` at an empty prompt inserts `:shell` and submits it immediately — no
/// Enter required. A `ConditionalRule` added by
/// `wrap_edit_mode_with_conditional_rules` falls back to `InsertChar(';')`
/// when the buffer is not empty, preserving normal semicolon input.
pub fn add_shell_semicolon_keybinding(keybindings: &mut Keybindings) {
    let event = ReedlineEvent::Multiple(vec![
        ReedlineEvent::Edit(vec![EditCommand::InsertString(":shell".to_string())]),
        ReedlineEvent::Submit,
    ]);
    // Bind both NONE and SHIFT: on some platforms/layouts crossterm includes
    // SHIFT in the key event even when it is part of typing the character.
    keybindings.add_binding(KeyModifiers::NONE, KeyCode::Char(';'), event.clone());
    keybindings.add_binding(KeyModifiers::SHIFT, KeyCode::Char(';'), event);
}

/// Add keybindings for inserting text (like assignment and pipe operators).
///
/// Configurable via `editor.key_map` in config using crokey format.
/// Example: "alt-hyphen" = " <- ", "alt-p" = " |> "
pub fn add_key_map_keybindings(
    keybindings: &mut Keybindings,
    key_map: &BTreeMap<KeyCombination, String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use reedline::{EditCommand, KeyCode, KeyModifiers, ReedlineEvent};

    fn shell_event() -> ReedlineEvent {
        ReedlineEvent::Multiple(vec![
            ReedlineEvent::Edit(vec![EditCommand::InsertString(":shell".to_string())]),
            ReedlineEvent::Submit,
        ])
    }

    #[test]
    fn test_shell_semicolon_keybinding_none_modifier() {
        let mut kb = reedline::Keybindings::new();
        add_shell_semicolon_keybinding(&mut kb);
        assert_eq!(
            kb.find_binding(KeyModifiers::NONE, KeyCode::Char(';')),
            Some(shell_event()),
            "NONE+';' should map to the shell shortcut event"
        );
    }

    #[test]
    fn test_shell_semicolon_keybinding_shift_modifier() {
        let mut kb = reedline::Keybindings::new();
        add_shell_semicolon_keybinding(&mut kb);
        assert_eq!(
            kb.find_binding(KeyModifiers::SHIFT, KeyCode::Char(';')),
            Some(shell_event()),
            "SHIFT+';' should map to the same shell shortcut event for cross-layout support"
        );
    }
}
