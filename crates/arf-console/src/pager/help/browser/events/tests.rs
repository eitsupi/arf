use super::*;
use crate::pager::help::browser::tests::topic;
use crate::pager::help::search::SearchWorker;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use std::sync::Arc;

#[test]
fn candidate_movement_keeps_pending_search_and_only_moves_ready_candidates() {
    let topics: Arc<[arf_harp::help::HelpTopic]> = vec![
        topic("base", "mean", &[], "Mean"),
        topic("stats", "median", &[], "Median"),
    ]
    .into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "m");
    browser.query_generation = 3;
    browser.pending_generation = Some(12);
    browser.remember_pending_open();
    browser.move_selection(1, 5);
    assert_eq!(browser.pending_generation, Some(12));
    assert_eq!(browser.pending_open, None);
    assert_eq!(browser.selected, 0);
    browser.accept_search_result(crate::pager::help::search::SearchResult {
        generation: 12,
        matches: vec![(0, 8), (1, 4)],
    });
    browser.move_selection(1, 5);
    assert_eq!(browser.selected, 1);
}

#[test]
fn ready_enter_stops_event_drain_before_later_browser_keys() {
    use std::collections::VecDeque;
    let topics: Arc<[arf_harp::help::HelpTopic]> = vec![topic("base", "mean", &[], "Mean")].into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "");
    let mut worker = SearchWorker::spawn(topics).unwrap();
    let mut events = VecDeque::from([
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
    ]);
    let drained = drain_help_events(
        || Ok(events.pop_front()),
        |event| browser.handle_event(event, false, &worker),
    )
    .unwrap();
    assert_eq!(drained.action, BrowserAction::Open(0));
    assert_eq!(drained.events_read, 1);
    assert_eq!(events.len(), 1, "the pager must receive the unread q key");
    worker.shutdown_and_join().unwrap();
}

#[test]
fn thirty_third_exit_cancels_pending_open_before_results_apply() {
    use std::collections::VecDeque;
    let topics: Arc<[arf_harp::help::HelpTopic]> = vec![topic("base", "mean", &[], "Mean")].into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "mean");
    browser.query_generation = 5;
    browser.pending_generation = Some(11);
    let mut worker = SearchWorker::spawn(topics).unwrap();
    let mut events = VecDeque::new();
    events.push_back(Event::Key(KeyEvent::new(
        KeyCode::Enter,
        KeyModifiers::NONE,
    )));
    events.extend((0..31).map(|_| Event::Resize(80, 24)));
    events.push_back(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
    let first = drain_help_events(
        || Ok(events.pop_front()),
        |event| browser.handle_event(event, false, &worker),
    )
    .unwrap();
    assert_eq!(first.action, BrowserAction::Continue);
    assert_eq!(first.events_read, 32);
    assert_eq!(browser.pending_open, Some(5));
    let mut applied = false;
    let backlog = poll_backlog_before_results(
        first.action,
        || Ok(!events.is_empty()),
        || {
            applied = true;
            Ok(())
        },
    )
    .unwrap();
    assert!(backlog);
    assert!(!applied);
    let second = drain_help_events(
        || Ok(events.pop_front()),
        |event| browser.handle_event(event, false, &worker),
    )
    .unwrap();
    assert_eq!(second.action, BrowserAction::Exit);
    assert_eq!(browser.pending_open, None);
    worker.shutdown_and_join().unwrap();
}

#[test]
fn input_arriving_at_end_of_redraw_prevents_result_application() {
    use std::collections::VecDeque;
    let mut events = VecDeque::new();
    let mut applied = false;
    let backlog = poll_backlog_before_results(
        BrowserAction::Continue,
        || {
            events.push_back(Event::Key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
            Ok(!events.is_empty())
        },
        || {
            applied = true;
            Ok(())
        },
    )
    .unwrap();
    assert!(backlog);
    assert!(!applied);
    assert_eq!(events.len(), 1);
}

#[test]
fn batched_edits_submit_only_final_query_for_deferred_enter() {
    use std::collections::VecDeque;
    let topics: Arc<[arf_harp::help::HelpTopic]> = vec![topic("base", "mean", &[], "Mean")].into();
    let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "");
    let mut worker = SearchWorker::spawn(topics).unwrap();
    let mut events = VecDeque::from([
        Event::Key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE)),
        Event::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
    ]);
    let drained = drain_help_events(
        || Ok(events.pop_front()),
        |event| browser.handle_event(event, false, &worker),
    )
    .unwrap();
    assert_eq!(drained.action, BrowserAction::Continue);
    assert_eq!(browser.query, "me");
    assert_eq!(browser.pending_open, Some(browser.query_generation));
    browser.dispatch_search(&worker).unwrap();
    let generation = browser.pending_generation.unwrap();
    assert_eq!(generation, 1, "one batch submits one final query");
    let result = worker.wait_for_result(generation).unwrap();
    assert_eq!(result.matches.first().map(|(index, _)| *index), Some(0));
    assert_eq!(browser.accept_search_result(result), Some(0));
    worker.shutdown_and_join().unwrap();
}
