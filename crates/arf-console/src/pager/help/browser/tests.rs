use super::*;
use crate::pager::help::search::SearchResult;
use arf_harp::help::{HelpTargetResolver, HelpTopic, get_help_topics_from_paths};
use std::sync::Arc;

pub(in crate::pager::help::browser) fn topic(
    package: &str,
    name: &str,
    aliases: &[&str],
    title: &str,
) -> HelpTopic {
    HelpTopic {
        package: package.to_owned(),
        package_dir: std::path::PathBuf::from(format!("/{package}")),
        topic: name.to_owned(),
        aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
        help_key: None,
        title: title.to_owned(),
        entry_type: "help".to_owned(),
    }
}

#[test]
fn pending_enter_opens_only_the_first_match_from_its_generation() {
    let topics: Arc<[HelpTopic]> = vec![topic("base", "mean", &[], "Mean")].into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "mea");
    browser.query_generation = 7;
    browser.pending_generation = Some(7);
    browser.remember_pending_open();

    assert_eq!(browser.pending_open, Some(7));
    assert_eq!(
        browser.accept_search_result(SearchResult {
            generation: 7,
            matches: vec![(0, 12)],
        }),
        Some(0)
    );
    assert_eq!(browser.pending_generation, None);
    assert_eq!(browser.filtered, [(0, 12)]);
}

#[test]
fn boundary_edits_clear_pending_open_without_cancelling_search() {
    let topics: Arc<[HelpTopic]> = vec![topic("base", "mean", &[], "Mean")].into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "mean");
    browser.query_generation = 17;
    browser.pending_generation = Some(41);
    browser.remember_pending_open();
    browser.cursor_pos = 0;
    browser.backspace_query();
    assert_eq!(browser.query, "mean");
    assert_eq!(browser.pending_generation, Some(41));
    assert_eq!(browser.pending_open, None);

    browser.remember_pending_open();
    browser.cursor_pos = browser.query.chars().count();
    browser.delete_query_char();
    assert_eq!(browser.query, "mean");
    assert_eq!(browser.pending_generation, Some(41));
    assert_eq!(browser.pending_open, None);
}

#[test]
fn stale_empty_result_cannot_open_and_query_edit_clears_pending_enter() {
    let topics: Arc<[HelpTopic]> = vec![topic("base", "mean", &[], "Mean")].into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "missing");
    browser.query_generation = 9;
    browser.pending_generation = Some(9);
    browser.remember_pending_open();
    assert_eq!(
        browser.accept_search_result(SearchResult {
            generation: 8,
            matches: vec![(0, 1)]
        }),
        None
    );
    browser.clear_pending_search();
    assert_eq!(browser.pending_open, None);
    assert_eq!(browser.pending_generation, None);
    browser.query_generation = 10;
    browser.pending_generation = Some(10);
    browser.remember_pending_open();
    assert_eq!(
        browser.accept_search_result(SearchResult {
            generation: 10,
            matches: Vec::new()
        }),
        None
    );
    assert!(browser.filtered.is_empty());
}

#[test]
fn non_empty_search_keeps_old_view_until_latest_result_replaces_it() {
    let topics: Arc<[HelpTopic]> = (0..4)
        .map(|index| topic("pkg", &format!("topic{index}"), &[], "title"))
        .collect::<Vec<_>>()
        .into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "old");
    browser.filtered = vec![(0, 10), (1, 9), (2, 8)];
    browser.selected = 2;
    browser.scroll_offset = 1;
    browser.text_scroll.scroll_pos = 6;
    browser.query = "new".to_owned();
    browser.cursor_pos = 3;

    browser.start_search();

    assert_eq!(browser.filtered, [(0, 10), (1, 9), (2, 8)]);
    assert_eq!(browser.selected, 2);
    assert_eq!(browser.scroll_offset, 1);
    assert_eq!(browser.text_scroll.scroll_pos, 6);
    assert!(browser.search_pending());

    browser.pending_generation = Some(23);
    assert_eq!(
        browser.accept_search_result(SearchResult {
            generation: 23,
            matches: vec![(3, 15)],
        }),
        None
    );
    assert_eq!(browser.filtered, [(3, 15)]);
    assert_eq!(browser.selected, 0);
    assert_eq!(browser.scroll_offset, 0);
    assert_eq!(browser.text_scroll.scroll_pos, 0);
}

#[test]
fn clearing_query_restores_initial_results_and_resets_view() {
    let topics: Arc<[HelpTopic]> = (0..3)
        .map(|index| topic("pkg", &format!("topic{index}"), &[], "title"))
        .collect::<Vec<_>>()
        .into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "x");
    browser.filtered = vec![(2, 10)];
    browser.selected = 4;
    browser.scroll_offset = 2;
    browser.text_scroll.scroll_pos = 5;

    browser.clear_query();

    assert_eq!(browser.filtered, [(0, 0), (1, 0), (2, 0)]);
    assert_eq!(browser.selected, 0);
    assert_eq!(browser.scroll_offset, 0);
    assert_eq!(browser.text_scroll.scroll_pos, 0);
}

#[test]
fn pending_selection_keys_do_not_open_or_move_old_results() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

    let topics: Arc<[HelpTopic]> = vec![topic("pkg", "old", &[], "Old")].into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "new");
    browser.filtered = vec![(0, 10)];
    browser.pending_generation = Some(4);
    browser.query_generation = 8;
    let mut worker = SearchWorker::spawn(topics).unwrap();

    for key in [KeyCode::Down, KeyCode::Enter, KeyCode::Tab] {
        let handled = browser
            .handle_event(
                Event::Key(KeyEvent::new(key, KeyModifiers::NONE)),
                false,
                &worker,
            )
            .unwrap();
        assert_eq!(handled.action, BrowserAction::Continue);
        assert_eq!(browser.selected, 0);
    }
    assert_eq!(browser.pending_open, Some(8));
    worker.shutdown_and_join().unwrap();
}

#[test]
fn query_edits_and_escape_cancel_without_waiting_for_search() {
    use std::sync::{Barrier, mpsc};

    let (started_tx, started_rx) = mpsc::channel();
    let gate = Arc::new(Barrier::new(2));
    let hook_gate = Arc::clone(&gate);
    let hook = Arc::new(move |generation| {
        let _ = started_tx.send(generation);
        hook_gate.wait();
    });
    let topics: Arc<[HelpTopic]> = vec![topic("base", "mean", &[], "Mean")].into();
    let mut worker = SearchWorker::spawn_with_hook(Arc::clone(&topics), hook).unwrap();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "x");
    let first_generation = worker.submit(browser.query.clone()).unwrap();
    browser.pending_generation = Some(first_generation);
    browser.remember_pending_open();
    assert_eq!(started_rx.recv().unwrap(), first_generation);

    browser.insert_query_char('y');
    assert_eq!(browser.query, "xy");
    assert_eq!(browser.pending_open, None);
    browser.backspace_query();
    assert_eq!(browser.query, "x");
    browser.dispatch_search(&worker).unwrap();
    let latest_generation = browser.pending_generation.unwrap();
    browser.remember_pending_open();
    browser.move_cursor(0);
    assert_eq!(browser.pending_generation, Some(latest_generation));
    assert!(browser.search_pending());
    browser.remember_pending_open();
    assert_eq!(browser.pending_open, Some(browser.query_generation));
    browser.move_cursor(browser.query.chars().count());
    assert_eq!(browser.pending_open, None);
    assert_eq!(browser.pending_generation, Some(latest_generation));
    browser.clear_query();
    assert_eq!(browser.query, "");
    assert_eq!(browser.filtered, [(0, 0)]);
    browser.dispatch_search(&worker).unwrap();
    browser.cancel_search(&worker).unwrap();
    gate.wait();
    worker.shutdown_and_join().unwrap();
    assert!(worker.take_result(first_generation).unwrap().is_none());
    assert!(worker.take_result(latest_generation).unwrap().is_none());
}

#[test]
fn help_library_snapshot_refresh_and_stale_fallback_policy() {
    use std::cell::Cell;
    let cached_read = Cell::new(false);
    let refreshed =
        crate::pager::help::help_library_paths_after_refresh(Ok(vec!["library-a".into()]), || {
            cached_read.set(true);
            vec!["library-b".into()]
        })
        .unwrap();
    assert_eq!(refreshed, ["library-a"]);
    assert!(!cached_read.get());
    let previous = crate::pager::help::help_library_paths_after_refresh(
        Err(arf_harp::HarpError::TypeMismatch {
            expected: "library path snapshot".into(),
            actual: "refresh failed".into(),
        }),
        || vec!["library-b".into()],
    )
    .unwrap();
    assert_eq!(previous, ["library-b"]);
    let error = crate::pager::help::help_library_paths_after_refresh(
        Err(arf_harp::HarpError::TypeMismatch {
            expected: "library path snapshot".into(),
            actual: "refresh failed".into(),
        }),
        Vec::new,
    )
    .expect_err("an empty cache cannot support help metadata discovery");
    assert!(matches!(error, arf_harp::HarpError::TypeMismatch { .. }));
}

fn navigation_fixture(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/help_navigation")
        .join(name)
}

fn install_navigation_package(library: &std::path::Path, name: &str) -> std::path::PathBuf {
    let package_dir = library.join(name);
    std::fs::create_dir_all(package_dir.join("Meta")).unwrap();
    std::fs::create_dir_all(package_dir.join("help")).unwrap();
    for extension in ["rdx", "rdb"] {
        std::fs::copy(
            navigation_fixture(&format!("resolverpkg.{extension}")),
            package_dir.join(format!("help/{name}.{extension}")),
        )
        .unwrap();
    }
    std::fs::copy(
        navigation_fixture("aliases.rds"),
        package_dir.join("help/aliases.rds"),
    )
    .unwrap();
    std::fs::copy(
        navigation_fixture("Rd.rds"),
        package_dir.join("Meta/Rd.rds"),
    )
    .unwrap();
    std::fs::write(package_dir.join("Meta/package.rds"), []).unwrap();
    package_dir
}

#[test]
fn help_snapshot_flows_from_metadata_through_browser_page_to_viewer_resolver() {
    use crate::pager::{PagerAction, PagerContent};
    use crossterm::event::{KeyCode, KeyModifiers};

    let temp = tempfile::tempdir().unwrap();
    let library_a = temp.path().join("library-a");
    let library_b = temp.path().join("library-b");
    let source_a = install_navigation_package(&library_a, "resolverpkg");
    install_navigation_package(&library_b, "resolverpkg");
    let target_a = install_navigation_package(&library_a, "targetpkg");
    let target_b = install_navigation_package(&library_b, "targetpkg");
    std::fs::write(target_b.join("help/targetpkg.rdx"), b"broken copy").unwrap();
    let snapshot = vec![
        library_a.to_string_lossy().into_owned(),
        library_b.to_string_lossy().into_owned(),
    ];
    let topics: Arc<[HelpTopic]> = get_help_topics_from_paths(&snapshot).into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), snapshot, "resolverpkg");
    browser.filtered = crate::help_search::search_topics_sync(&topics, "resolverpkg");
    let selected_index = browser
        .filtered
        .iter()
        .position(|(index, _)| {
            let topic = &topics[*index];
            topic.package == "resolverpkg"
                && topic.package_dir == source_a
                && topic.help_key.is_some()
        })
        .expect("first installed package copy should appear in metadata results");
    browser.selected = selected_index;
    let topic = &topics[browser.filtered[browser.selected].0];
    let mut page = HelpTargetResolver::prepare_candidate(topic).unwrap();
    assert_eq!(page.package_dir, source_a);
    page.markdown = "[target](x-r-help:targetpkg/lm)".to_owned();
    let mut viewer = crate::pager::help_session::HelpViewer::new(
        vec![page],
        browser.library_paths.clone(),
        40,
        8,
    );
    assert_eq!(viewer.title(), Some("resolverpkg::mean"));
    assert_eq!(
        PagerContent::handle_key(&mut viewer, KeyCode::Tab, KeyModifiers::NONE),
        Some(PagerAction::Redraw)
    );
    assert_eq!(
        PagerContent::handle_key(&mut viewer, KeyCode::Enter, KeyModifiers::NONE),
        Some(PagerAction::ScrollTo(0))
    );
    assert_eq!(viewer.title(), Some("targetpkg::lm"));
    assert_ne!(target_a, target_b);
    assert!(
        !PagerContent::feedback_message(&viewer)
            .unwrap_or_default()
            .contains("Unable to open")
    );
}
