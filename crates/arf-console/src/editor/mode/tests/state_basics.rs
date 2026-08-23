use super::super::*;

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
