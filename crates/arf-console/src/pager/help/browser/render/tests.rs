use super::*;
use crate::pager::help::browser::tests::topic;
use arf_harp::help::HelpTopic;
use std::sync::Arc;

#[test]
fn test_calculate_layout_standard() {
    assert_eq!(calculate_layout(80), (26, 49));
}

#[test]
fn test_calculate_layout_wide() {
    assert_eq!(calculate_layout(120), (40, 75));
}

#[test]
fn test_calculate_layout_narrow() {
    assert_eq!(calculate_layout(60), (20, 35));
}

#[test]
fn test_calculate_layout_very_narrow() {
    assert_eq!(calculate_layout(40), (20, 15));
}

#[test]
fn pending_search_renders_old_candidates_and_only_shows_placeholder_when_empty() {
    let topics: Arc<[HelpTopic]> = vec![
        topic("pkg", "old_first", &[], "First old title"),
        topic("pkg", "old_selected", &[], "Selected old title"),
    ]
    .into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "new");
    browser.filtered = vec![(0, 12), (1, 10)];
    browser.selected = 1;
    browser.text_scroll.scroll_pos = 4;
    browser.start_search();

    let mut frame = Vec::new();
    browser.render_to(&mut frame, 80, 12).unwrap();
    let rendered = String::from_utf8(frame).unwrap();
    assert!(rendered.contains("2 topics"));
    assert!(rendered.contains("old_first"));
    assert!(rendered.contains("old_selected"));
    assert!(!rendered.contains("Searching"));

    browser.filtered.clear();
    let mut empty_frame = Vec::new();
    browser.render_to(&mut empty_frame, 80, 12).unwrap();
    let rendered = String::from_utf8(empty_frame).unwrap();
    assert!(rendered.contains("Searching"));
}
