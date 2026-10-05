use super::*;
use std::path::{Path, PathBuf};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/help_navigation")
        .join(name)
}

fn package(library: &Path, name: &str, home: bool) -> PathBuf {
    let dir = library.join(name);
    std::fs::create_dir_all(dir.join("help")).unwrap();
    std::fs::create_dir_all(dir.join("Meta")).unwrap();
    let stem = if home { "homepkg" } else { "resolverpkg" };
    for ext in ["rdx", "rdb"] {
        std::fs::copy(
            fixture(&format!("{stem}.{ext}")),
            dir.join(format!("help/{name}.{ext}")),
        )
        .unwrap();
    }
    let aliases = if home {
        "home_aliases.rds"
    } else {
        "aliases.rds"
    };
    std::fs::copy(fixture(aliases), dir.join("help/aliases.rds")).unwrap();
    if !home {
        std::fs::copy(fixture("Rd.rds"), dir.join("Meta/Rd.rds")).unwrap();
    }
    std::fs::write(dir.join("Meta/package.rds"), []).unwrap();
    dir
}

fn home(dir: &Path, markdown: &str) -> PreparedHelpPage {
    PreparedHelpPage {
        package_dir: dir.to_owned(),
        package: "source".to_owned(),
        display_topic: "home".to_owned(),
        help_key: "home".to_owned(),
        markdown: markdown.to_owned(),
    }
}

fn viewer(page: PreparedHelpPage, library: &Path) -> HelpViewer {
    HelpViewer::new(
        vec![page],
        vec![library.to_str().unwrap().to_owned()],
        40,
        8,
    )
}

/// Apply the pager's clamped viewport behavior after each state transition.
fn key(viewer: &mut HelpViewer, code: KeyCode, modifiers: KeyModifiers) -> Option<PagerAction> {
    let action = viewer.handle_key(code, modifiers);
    let offset = match action {
        Some(PagerAction::ScrollTo(offset)) => {
            super::super::clamp_scroll_offset(offset, viewer.line_count(), viewer.height)
        }
        _ => viewer.scroll_offset,
    };
    viewer.prepare_render(offset);
    action
}

fn press(viewer: &mut HelpViewer, code: KeyCode) -> Option<PagerAction> {
    key(viewer, code, KeyModifiers::NONE)
}

fn search(viewer: &mut HelpViewer, text: &str) {
    press(viewer, KeyCode::Char('/'));
    for ch in text.chars() {
        press(viewer, KeyCode::Char(ch));
    }
    press(viewer, KeyCode::Enter);
}

#[test]
fn follow_and_back_restore_page_provenance_title_scroll_focus_and_search() {
    let temp = tempfile::tempdir().unwrap();
    let origin = package(temp.path(), "source", true);
    let other = package(temp.path(), "other", false);
    let source = format!(
        "{}\n\nRead [lm](x-r-help:other/lm).",
        "paragraph\n\n".repeat(12)
    );
    let original = home(&origin, &source);
    let mut viewer = viewer(original.clone(), temp.path());
    assert_eq!(
        press(&mut viewer, KeyCode::Enter),
        Some(PagerAction::Redraw)
    );
    assert!(viewer.history.is_empty());
    press(&mut viewer, KeyCode::Tab);
    assert!(viewer.scroll_offset > 0);
    search(&mut viewer, "lm");
    let scroll = viewer.scroll_offset;
    let status = viewer.feedback_message().unwrap().to_owned();
    let lines: Vec<_> = (0..viewer.line_count())
        .map(|i| viewer.render_line(i, 40))
        .collect();

    press(&mut viewer, KeyCode::Enter);
    assert_eq!(viewer.title(), Some("other::lm"));
    assert_eq!(viewer.current.as_ref().unwrap().page.package_dir, other);
    assert_eq!(
        viewer.current.as_ref().unwrap().page.help_key,
        "second-topic"
    );
    assert_eq!(viewer.scroll_offset, 0);
    assert_eq!(viewer.history.len(), 1);
    assert!(!viewer.feedback_message().unwrap().contains("/lm ["));

    press(&mut viewer, KeyCode::Backspace);
    assert_eq!(viewer.title(), Some("source::home"));
    assert_eq!(viewer.current.as_ref().unwrap().page, original);
    assert_eq!(viewer.scroll_offset, scroll);
    assert_eq!(viewer.feedback_message(), Some(status.as_str()));
    assert_eq!(
        viewer
            .current
            .as_ref()
            .unwrap()
            .content
            .selected_target()
            .unwrap()
            .topic,
        "lm"
    );
    let restored: Vec<_> = (0..viewer.line_count())
        .map(|i| viewer.render_line(i, 40))
        .collect();
    assert_eq!(restored, lines);
    assert!(viewer.history.is_empty());
    assert_eq!(
        key(&mut viewer, KeyCode::Left, KeyModifiers::ALT),
        Some(PagerAction::Redraw)
    );
    press(&mut viewer, KeyCode::Enter);
    key(&mut viewer, KeyCode::Left, KeyModifiers::ALT);
    assert_eq!(viewer.title(), Some("source::home"));
}

#[test]
fn unqualified_operator_link_uses_the_current_installed_copy() {
    let temp = tempfile::tempdir().unwrap();
    let early = temp.path().join("early");
    let late = temp.path().join("late");
    let shadow = package(&early, "source", false);
    let origin = package(&late, "source", false);
    std::fs::write(shadow.join("help/source.rdx"), b"broken shadow copy").unwrap();
    let mut viewer = viewer(home(&origin, "[divide](x-r-help:/%/%)"), &early);
    press(&mut viewer, KeyCode::Tab);
    press(&mut viewer, KeyCode::Enter);
    assert_eq!(viewer.title(), Some("source::%/%"));
    assert_eq!(viewer.current.as_ref().unwrap().page.package_dir, origin);
    assert_eq!(
        viewer.current.as_ref().unwrap().page.help_key,
        "first-topic"
    );
}

#[test]
fn ambiguous_follow_requires_selection_cancel_keeps_page_and_selected_candidate_keeps_identity() {
    let temp = tempfile::tempdir().unwrap();
    let origin = package(temp.path(), "source", true);
    let first = package(temp.path(), "first", false);
    let second = package(temp.path(), "second", false);
    let source = format!(
        "{}\n\n[shared](x-r-help:/shared)",
        "paragraph\n\n".repeat(10)
    );
    let original = home(&origin, &source);
    let mut viewer = viewer(original.clone(), temp.path());
    press(&mut viewer, KeyCode::Tab);
    let offset = viewer.scroll_offset;
    let selected = viewer.current.as_ref().unwrap().content.selected_target();
    press(&mut viewer, KeyCode::Enter);
    assert_eq!(viewer.title(), Some("Select R help page"));
    assert_eq!(viewer.line_count(), 2);
    assert!(
        viewer
            .render_line(0, 40)
            .to_string()
            .contains(&first.display().to_string())
    );
    assert!(
        viewer
            .render_line(1, 40)
            .to_string()
            .contains(&second.display().to_string())
    );
    press(&mut viewer, KeyCode::Enter);
    assert!(viewer.candidates.is_some());
    assert!(viewer.history.is_empty());
    press(&mut viewer, KeyCode::Esc);
    assert_eq!(viewer.current.as_ref().unwrap().page, original);
    assert_eq!(viewer.scroll_offset, offset);
    assert_eq!(
        viewer.current.as_ref().unwrap().content.selected_target(),
        selected
    );

    press(&mut viewer, KeyCode::Enter);
    press(&mut viewer, KeyCode::Down);
    press(&mut viewer, KeyCode::Down);
    assert_eq!(viewer.candidates.as_ref().unwrap().state.selected, Some(1));
    press(&mut viewer, KeyCode::Enter);
    assert_eq!(viewer.title(), Some("second::shared"));
    assert_eq!(viewer.current.as_ref().unwrap().page.package_dir, second);
    press(&mut viewer, KeyCode::Backspace);
    assert_eq!(viewer.current.as_ref().unwrap().page, original);
    assert_eq!(viewer.scroll_offset, offset);
}

#[test]
fn not_found_corrupt_database_and_failed_candidate_preserve_original_page() {
    let temp = tempfile::tempdir().unwrap();
    let origin = package(temp.path(), "source", true);
    let first = package(temp.path(), "first", false);
    package(temp.path(), "second", false);
    let source = "[missing](x-r-help:absent/nope) [broken](x-r-help:first/mean) [ambiguous](x-r-help:/shared)";
    let original = home(&origin, source);
    let mut viewer = viewer(original.clone(), temp.path());
    press(&mut viewer, KeyCode::Tab);
    press(&mut viewer, KeyCode::Enter);
    assert!(viewer.feedback_message().unwrap().contains("not found"));
    assert_eq!(viewer.current.as_ref().unwrap().page, original);
    press(&mut viewer, KeyCode::Tab);
    std::fs::write(first.join("help/first.rdx"), b"broken database").unwrap();
    press(&mut viewer, KeyCode::Enter);
    assert!(
        viewer
            .feedback_message()
            .unwrap()
            .contains("Unable to open")
    );
    assert_eq!(viewer.current.as_ref().unwrap().page, original);
    press(&mut viewer, KeyCode::Tab);
    let offset = viewer.scroll_offset;
    press(&mut viewer, KeyCode::Enter);
    press(&mut viewer, KeyCode::Down);
    press(&mut viewer, KeyCode::Enter);
    assert!(viewer.candidates.is_none());
    assert!(
        viewer
            .feedback_message()
            .unwrap()
            .contains("Unable to open")
    );
    assert_eq!(viewer.current.as_ref().unwrap().page, original);
    assert_eq!(viewer.scroll_offset, offset);
    assert!(viewer.history.is_empty());
}

#[test]
fn initial_multiple_pages_share_the_loop_and_require_confirmation() {
    let temp = tempfile::tempdir().unwrap();
    let first = home(temp.path(), "First");
    let second = PreparedHelpPage {
        package: "other".to_owned(),
        markdown: "Second".to_owned(),
        ..first.clone()
    };
    let mut viewer = HelpViewer::new(vec![first.clone(), second.clone()], vec![], 40, 8);
    assert_eq!(viewer.title(), Some("Select R help page"));
    assert_eq!(
        press(&mut viewer, KeyCode::Enter),
        Some(PagerAction::Redraw)
    );
    assert_eq!(
        press(&mut viewer, KeyCode::Char('q')),
        Some(PagerAction::Exit)
    );
    assert!(viewer.current.is_none());

    let mut viewer = HelpViewer::new(vec![first, second.clone()], vec![], 40, 8);
    press(&mut viewer, KeyCode::Down);
    press(&mut viewer, KeyCode::Down);
    press(&mut viewer, KeyCode::Enter);
    assert_eq!(viewer.current.as_ref().unwrap().page, second);
    assert!(viewer.history.is_empty());
}

#[test]
fn candidate_selection_scrolls_to_hidden_rows_and_resize_cancel_restores_the_page() {
    let temp = tempfile::tempdir().unwrap();
    let original = home(temp.path(), "First");
    let pages = (0..15)
        .map(|index| PreparedHelpPage {
            display_topic: format!("topic-{index}"),
            ..original.clone()
        })
        .collect();
    let mut viewer = HelpViewer::new(pages, vec![], 40, 6);
    for _ in 0..15 {
        press(&mut viewer, KeyCode::Down);
    }
    assert_eq!(viewer.candidates.as_ref().unwrap().state.selected, Some(14));
    assert_eq!(viewer.scroll_offset, 11);
    press(&mut viewer, KeyCode::Enter);
    assert_eq!(viewer.title(), Some("source::topic-14"));
    assert_eq!(viewer.scroll_offset, 0);

    let origin = package(temp.path(), "source", true);
    package(temp.path(), "first", false);
    package(temp.path(), "second", false);
    let source = format!(
        "{}\n\n[shared](x-r-help:/shared)",
        "A paragraph with several words to wrap.\n\n".repeat(10)
    );
    let mut viewer = HelpViewer::new(
        vec![home(&origin, &source)],
        vec![temp.path().to_str().unwrap().to_owned()],
        40,
        8,
    );
    press(&mut viewer, KeyCode::Tab);
    press(&mut viewer, KeyCode::Enter);
    viewer.on_resize(16, 6);
    assert_eq!(viewer.take_scroll_request(), None);
    press(&mut viewer, KeyCode::Esc);
    assert_eq!(viewer.title(), Some("source::home"));
    assert!(viewer.scroll_offset > 0);
    assert!(
        viewer.scroll_offset <= super::super::max_scroll_offset_with_height(viewer.line_count(), 6)
    );
    assert_eq!(
        viewer
            .current
            .as_ref()
            .unwrap()
            .content
            .selected_target()
            .unwrap()
            .topic,
        "shared"
    );
}

#[test]
fn footer_only_advertises_available_link_actions_and_keeps_exit_visible_at_narrow_widths() {
    let temp = tempfile::tempdir().unwrap();
    let mut viewer = viewer(home(temp.path(), "[mean](x-r-help:/mean)"), temp.path());
    assert!(viewer.feedback_message().unwrap().contains("Tab"));
    assert!(!viewer.feedback_message().unwrap().contains("Enter"));
    press(&mut viewer, KeyCode::Tab);
    assert!(viewer.feedback_message().unwrap().contains("Enter open"));
    viewer.on_resize(30, 8);
    assert!(viewer.feedback_message().unwrap().starts_with("q/Esc exit"));
    let mut plain = HelpViewer::new(
        vec![home(temp.path(), "[website](https://example.com)")],
        vec![],
        80,
        24,
    );
    assert!(!plain.feedback_message().unwrap().contains("Tab"));
    assert_eq!(press(&mut plain, KeyCode::Tab), Some(PagerAction::Redraw));
    assert_eq!(press(&mut plain, KeyCode::Enter), Some(PagerAction::Redraw));
}

#[test]
fn search_input_consumes_link_and_history_keys_and_emergency_exit_falls_through() {
    let temp = tempfile::tempdir().unwrap();
    let mut viewer = viewer(home(temp.path(), "[mean](x-r-help:/mean)"), temp.path());
    press(&mut viewer, KeyCode::Tab);
    let target = viewer.current.as_ref().unwrap().content.selected_target();
    press(&mut viewer, KeyCode::Char('/'));
    press(&mut viewer, KeyCode::Char('q'));
    for code in [KeyCode::Tab, KeyCode::BackTab, KeyCode::Backspace] {
        press(&mut viewer, code);
    }
    key(&mut viewer, KeyCode::Left, KeyModifiers::ALT);
    assert_eq!(
        viewer.feedback_message(),
        Some("/|  Enter search  Esc cancel")
    );
    press(&mut viewer, KeyCode::Char('m'));
    press(&mut viewer, KeyCode::Enter);
    assert_eq!(viewer.title(), Some("source::home"));
    assert_eq!(
        viewer.current.as_ref().unwrap().content.selected_target(),
        target
    );
    assert!(viewer.feedback_message().unwrap().contains("/m [1/"));
    assert_eq!(
        press(&mut viewer, KeyCode::Char('q')),
        Some(PagerAction::Redraw)
    );
    assert_eq!(press(&mut viewer, KeyCode::Char('q')), None);
    press(&mut viewer, KeyCode::Char('/'));
    assert_eq!(
        key(&mut viewer, KeyCode::Char('c'), KeyModifiers::CONTROL),
        None
    );
    assert_eq!(
        key(&mut viewer, KeyCode::Char('d'), KeyModifiers::CONTROL),
        None
    );
    assert_eq!(press(&mut viewer, KeyCode::Esc), Some(PagerAction::Redraw));
    assert_eq!(press(&mut viewer, KeyCode::Esc), None);
}

#[test]
fn back_after_resize_reflows_saved_page_and_preserves_query_and_link_identity() {
    let temp = tempfile::tempdir().unwrap();
    let origin = package(temp.path(), "source", true);
    package(temp.path(), "other", false);
    let source = format!(
        "{}\n\n[平均 with a long label](x-r-help:other/mean)",
        "A paragraph with several words to wrap.\n\n".repeat(8)
    );
    let mut viewer = viewer(home(&origin, &source), temp.path());
    press(&mut viewer, KeyCode::Tab);
    search(&mut viewer, "平均 with");
    press(&mut viewer, KeyCode::Enter);
    viewer.on_resize(16, 6);
    let offset = viewer.take_scroll_request().unwrap();
    let offset = super::super::clamp_scroll_offset(offset, viewer.line_count(), 6);
    viewer.prepare_render(offset);
    assert_eq!(viewer.take_scroll_request(), None);
    press(&mut viewer, KeyCode::Backspace);
    assert_eq!(viewer.title(), Some("source::home"));
    assert!(
        viewer
            .feedback_message()
            .unwrap()
            .contains("/平均 with [0/1]")
    );
    assert_eq!(
        viewer
            .current
            .as_ref()
            .unwrap()
            .content
            .selected_target()
            .unwrap()
            .topic,
        "mean"
    );
    assert!(
        viewer.scroll_offset <= super::super::max_scroll_offset_with_height(viewer.line_count(), 6)
    );
    assert!(viewer.scroll_offset > 0);
    assert!((0..viewer.line_count()).any(|i| {
        viewer
            .render_line(i, 16)
            .spans
            .iter()
            .any(|s| s.style.bg == Some(Color::Cyan))
    }));
    press(&mut viewer, KeyCode::Char('n'));
    assert!(
        viewer
            .feedback_message()
            .unwrap()
            .contains("/平均 with [1/1]")
    );
}
