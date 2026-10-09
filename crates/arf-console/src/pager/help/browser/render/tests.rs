use super::*;
use crate::pager::text_utils::{exceeds_width, scroll_display, truncate_to_width};

#[test]
fn test_truncate_to_width_no_truncation() {
    assert_eq!(truncate_to_width("Hello", 10), "Hello");
    assert_eq!(truncate_to_width("Hello", 5), "Hello");
}

#[test]
fn test_truncate_to_width_with_truncation() {
    assert_eq!(truncate_to_width("Hello World", 8), "Hello W…");
    assert_eq!(truncate_to_width("Hello World", 6), "Hello…");
}

#[test]
fn test_truncate_to_width_edge_cases() {
    assert_eq!(truncate_to_width("Hi", 1), "…");
    assert_eq!(truncate_to_width("Hi", 0), "");
    assert_eq!(truncate_to_width("", 5), "");
}

#[test]
fn test_truncate_to_width_unicode() {
    assert_eq!(truncate_to_width("日本語テスト", 7), "日本語…");
    assert_eq!(truncate_to_width("日本語", 10), "日本語");
}

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
fn test_exceeds_width() {
    assert!(!exceeds_width("Hello", 10));
    assert!(!exceeds_width("Hello", 5));
    assert!(exceeds_width("Hello World", 8));
    assert!(exceeds_width("Hello", 4));
}

#[test]
fn test_scroll_display_no_truncation() {
    assert_eq!(scroll_display("Hello", 10, 0), ("Hello".to_owned(), 0));
}

#[test]
fn test_scroll_display_at_start() {
    assert_eq!(
        scroll_display("Hello World", 8, 0),
        ("Hello W…".to_owned(), 4)
    );
}

#[test]
fn test_scroll_display_at_end() {
    assert_eq!(scroll_display("Hello World", 8, 10).0, "…o World");
}

#[test]
fn test_scroll_display_in_middle() {
    assert_eq!(scroll_display("Hello World", 8, 2).0, "…llo Wo…");
}

#[test]
fn test_scroll_display_unicode() {
    assert_eq!(
        scroll_display("日本語テスト", 7, 0),
        ("日本語…".to_owned(), 6)
    );
    assert_eq!(scroll_display("日本語テスト", 7, 100).0, "…テスト");
}
