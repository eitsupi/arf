use super::*;
use std::io;

#[test]
fn prepared_help_selector_requires_an_explicit_selection_and_confirmation() {
    let mut selector = HelpPageSelectorState::new(2);
    assert_eq!(selector.selected, None);
    assert_eq!(selector.confirm(), None);
    assert_eq!(selector.confirmed, None);
    assert_eq!(selector.move_down(), Some(true));
    assert_eq!(selector.selected, Some(0));
    assert_eq!(selector.move_down(), Some(false));
    assert_eq!(selector.selected, Some(1));
    assert_eq!(selector.confirm(), Some(1));
    assert_eq!(selector.confirmed, Some(1));
}

#[test]
fn prepared_help_selector_navigation_stays_within_candidate_bounds() {
    let mut empty = HelpPageSelectorState::new(0);
    assert!(!empty.move_up());
    assert_eq!(empty.selected, None);
    let mut selector = HelpPageSelectorState::new(2);
    assert!(selector.move_up());
    assert_eq!(selector.selected, Some(1));
    assert!(selector.move_up());
    assert_eq!(selector.selected, Some(0));
    assert!(selector.move_up());
    assert_eq!(selector.selected, None);
    assert_eq!(selector.move_down(), Some(true));
    assert!(selector.move_up());
    assert_eq!(selector.selected, None);
    assert_eq!(selector.move_down(), Some(true));
    assert_eq!(selector.move_down(), Some(false));
    assert_eq!(selector.move_down(), None);
    assert_eq!(selector.selected, Some(1));
}

#[test]
fn help_page_title_uses_package_and_display_topic() {
    assert_eq!(
        help_page_title("base", "[.data.frame"),
        "base::[.data.frame"
    );
}

#[test]
fn help_page_load_error_message_includes_a_concise_context_and_detail() {
    let error = io::Error::other("compiled topic was not found");
    assert_eq!(
        help_page_load_error_message(&error),
        "Unable to load this help topic.\n\ncompiled topic was not found"
    );
}
