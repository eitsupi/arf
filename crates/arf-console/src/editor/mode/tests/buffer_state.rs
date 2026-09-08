use super::super::*;

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
    state.cursor_pos = 1;

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
    state.cursor_pos = 2;

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
    state.cursor_pos = 1;

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
    state.cursor_pos = 6;

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
    state.cursor_pos = 5;

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
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('日')]));
    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::InsertChar('本')]));
    assert_eq!(state.buffer, "日本");
    assert_eq!(state.cursor_pos, 2);

    state.update_from_event(&ReedlineEvent::Edit(vec![EditCommand::Backspace]));
    assert_eq!(state.buffer, "日");
    assert_eq!(state.cursor_pos, 1);
    assert_eq!(state.buffer_len, 1);
}
