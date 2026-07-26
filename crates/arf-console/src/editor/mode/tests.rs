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
fn test_buffer_known_empty_condition() {
    let condition = BufferKnownEmpty;

    let mut state = EditorState::new();
    state.buffer_len = 0;
    state.uncertain = false;
    assert!(
        condition.check(&state),
        "certain empty buffer should trigger semicolon shortcut condition"
    );

    state.uncertain = true;
    assert!(
        !condition.check(&state),
        "uncertain empty buffer should NOT trigger semicolon shortcut condition"
    );

    state.buffer_len = 1;
    state.uncertain = false;
    assert!(
        !condition.check(&state),
        "non-empty buffer should not match"
    );
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
    let multiple_event = ReedlineEvent::Edit(vec![EditCommand::Backspace, EditCommand::Delete]);
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
    let pairs = [
        ("()", 1),
        ("[]", 1),
        ("{}", 1),
        (r#""""#, 1),
        ("''", 1),
        ("``", 1),
    ];

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

        assert_eq!(
            state.buffer, "",
            "Buffer should be empty after deleting {}",
            pair
        );
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

// ===== Tests for skip-over rules =====

#[test]
fn test_skip_over_rules_created() {
    let rules = create_skip_over_rules();

    // Should have 6 rules: 3 for brackets (`)`, `]`, `}`) + 3 for quotes (`"`, `'`, `` ` ``)
    assert_eq!(rules.len(), 6);
}

/// Test that bracket skip-over rules match InsertChar events (not InsertString)
///
/// This is critical: bracket closing keys (`)`, `]`, `}`) generate InsertChar events,
/// while opening keys (`(`, `[`, `{`) generate InsertString + MoveLeft.
/// Skip-over rules must only match InsertChar to avoid interfering with opening brackets.
#[test]
fn test_skip_over_bracket_rules_match_insert_char() {
    let rules = create_skip_over_rules();

    // First 3 rules are for brackets: `)`, `]`, `}`
    for (i, close_char) in [')', ']', '}'].iter().enumerate() {
        let rule = &rules[i];

        // Should match InsertChar(close_char)
        let insert_char_event = ReedlineEvent::Edit(vec![EditCommand::InsertChar(*close_char)]);
        assert!(
            (rule.match_event)(&insert_char_event),
            "Rule {} should match InsertChar('{}')",
            i,
            close_char
        );

        // Should NOT match InsertString(pair) + MoveLeft (opening bracket event)
        let pair = match close_char {
            ')' => "()",
            ']' => "[]",
            '}' => "{}",
            _ => unreachable!(),
        };
        let insert_string_event = ReedlineEvent::Edit(vec![
            EditCommand::InsertString(pair.to_string()),
            EditCommand::MoveLeft { select: false },
        ]);
        assert!(
            !(rule.match_event)(&insert_string_event),
            "Rule {} should NOT match InsertString for pair '{}'",
            i,
            pair
        );
    }
}

/// Test that quote skip-over rules match InsertString + MoveLeft events
///
/// Quotes use the same character for opening and closing, so they use
/// InsertString(pair) + MoveLeft for both.
#[test]
fn test_skip_over_quote_rules_match_insert_string() {
    let rules = create_skip_over_rules();

    // Last 3 rules are for quotes: `"`, `'`, `` ` ``
    let quote_pairs: [(char, &str); 3] = [('"', r#""""#), ('\'', "''"), ('`', "``")];
    for (i, (quote_char, pair)) in quote_pairs.iter().enumerate() {
        let rule = &rules[3 + i]; // Skip first 3 bracket rules

        // Should match InsertString(pair) + MoveLeft
        let insert_string_event = ReedlineEvent::Edit(vec![
            EditCommand::InsertString(pair.to_string()),
            EditCommand::MoveLeft { select: false },
        ]);
        assert!(
            (rule.match_event)(&insert_string_event),
            "Quote rule {} should match InsertString for pair '{}'",
            i,
            pair
        );

        // Should NOT match InsertChar(quote_char)
        let insert_char_event = ReedlineEvent::Edit(vec![EditCommand::InsertChar(*quote_char)]);
        assert!(
            !(rule.match_event)(&insert_char_event),
            "Quote rule {} should NOT match InsertChar('{}')",
            i,
            quote_char
        );
    }
}

/// Regression test: opening bracket should insert pair even when cursor is before closing bracket
///
/// Bug scenario: `install.packages(c|)` -> type `(` -> should become `install.packages(c(|))`
/// Before fix: skip-over rule incorrectly matched the opening `(` key and moved cursor right
/// After fix: skip-over only matches InsertChar(')'), so `(` key works correctly
#[test]
fn test_opening_bracket_not_matched_by_skip_over_regression() {
    let skip_over_rules = create_skip_over_rules();
    let auto_match_rules = create_auto_match_rules();

    // The event generated when pressing `(` key (opening bracket)
    let open_paren_event = ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]);

    // Skip-over rule for `)` should NOT match the `(` key event
    let close_paren_skip_rule = &skip_over_rules[0]; // First rule is for `)`
    assert!(
        !(close_paren_skip_rule.match_event)(&open_paren_event),
        "Skip-over rule for ')' should NOT match opening '(' event"
    );

    // Auto-match rule for `(` should match the `(` key event
    let open_paren_auto_match_rule = &auto_match_rules[0]; // First rule is for `(`
    assert!(
        (open_paren_auto_match_rule.match_event)(&open_paren_event),
        "Auto-match rule for '(' should match opening '(' event"
    );
}

/// Test the full scenario: cursor before `)`, press `(`, should insert `()`
///
/// Simulates: `foo(c|)` -> press `(` -> `foo(c(|))`
#[test]
fn test_open_bracket_inside_parens_full_flow() {
    let mut state = EditorState::new();
    let skip_over_rules = create_skip_over_rules();
    let auto_match_rules = create_auto_match_rules();

    // Setup: `foo(c|)` - cursor before `)`
    state.buffer = "foo(c)".to_string();
    state.buffer_len = 6;
    state.cursor_pos = 5; // After 'c', before ')'

    assert_eq!(state.char_after_cursor(), Some(')'));
    assert!(!state.cursor_at_end());

    // The event for pressing `(` key
    let open_paren_event = ReedlineEvent::Edit(vec![
        EditCommand::InsertString("()".to_string()),
        EditCommand::MoveLeft { select: false },
    ]);

    // Skip-over rule for `)` should NOT match (it matches InsertChar, not InsertString)
    let close_paren_skip_rule = &skip_over_rules[0];
    assert!(!(close_paren_skip_rule.match_event)(&open_paren_event));

    // Auto-match rule for `(` should match
    let open_paren_auto_match_rule = &auto_match_rules[0];
    assert!((open_paren_auto_match_rule.match_event)(&open_paren_event));

    // Auto-match condition should pass (cursor is before closing char)
    assert!(
        open_paren_auto_match_rule.condition.check(&state),
        "Auto-match condition should pass when cursor is before ')'"
    );

    // Simulate the auto-match event execution
    state.update_from_event(&open_paren_event);

    // Result should be: `foo(c(|))`
    assert_eq!(state.buffer, "foo(c())");
    assert_eq!(state.cursor_pos, 6); // Between the new parens
}

/// Test that closing bracket skip-over works correctly
///
/// Simulates: `foo(|)` -> press `)` -> `foo()|`
#[test]
fn test_close_bracket_skip_over() {
    let mut state = EditorState::new();
    let skip_over_rules = create_skip_over_rules();

    // Setup: `foo(|)` - cursor before `)`
    state.buffer = "foo()".to_string();
    state.buffer_len = 5;
    state.cursor_pos = 4; // After '(', before ')'

    assert_eq!(state.char_after_cursor(), Some(')'));

    // The event for pressing `)` key (InsertChar)
    let close_paren_event = ReedlineEvent::Edit(vec![EditCommand::InsertChar(')')]);

    // Skip-over rule for `)` should match
    let close_paren_skip_rule = &skip_over_rules[0];
    assert!((close_paren_skip_rule.match_event)(&close_paren_event));

    // Condition: CursorNotBeforeChar(')') - should be FALSE because cursor IS before ')'
    // When condition is false, fallback (MoveRight) is used
    assert!(
        !close_paren_skip_rule.condition.check(&state),
        "Condition should fail when cursor is before ')'"
    );

    // The fallback event is MoveRight, which skips over the existing ')'
    // Simulate that
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::MoveRight {
        select: false,
    }]));

    assert_eq!(state.cursor_pos, 5); // Now at end
    assert_eq!(state.buffer, "foo()"); // Buffer unchanged
}
