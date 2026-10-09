//! Interactive help browser state and search lifecycle.

use super::search::{self, SearchWorker};
use super::{MAX_FILTERED_RESULTS, MIN_SIZE, help_library_paths_after_refresh, pages};
use crate::pager::{TextScrollState, check_terminal_too_small, with_alternate_screen};
use arf_harp::help::{
    HelpTargetResolver, HelpTopic, get_help_topics_from_paths, get_vignette_text,
};
use arf_harp::lib_paths::{cached_lib_paths, refresh_lib_paths_from_r};
use crossterm::event;
use std::io;
use std::sync::Arc;
use std::time::Duration;

mod events;
mod render;
#[cfg(test)]
mod tests;
use events::{BrowserAction, drain_help_events, poll_backlog_before_results};
use pages::{display_help_pager, display_help_pages, help_page_load_error_message};

/// Interactive help browser.
struct HelpBrowser {
    library_paths: Vec<String>,
    topics: Arc<[HelpTopic]>,
    query: String,
    /// Cursor position within the query string (in characters, not bytes).
    cursor_pos: usize,
    filtered: Vec<(usize, u32)>,
    pending_generation: Option<u64>,
    pending_open: Option<u64>,
    query_generation: u64,
    search_dirty: bool,
    selected: usize,
    scroll_offset: usize,
    /// Scroll animation state for the selected item's long text.
    text_scroll: TextScrollState,
}

struct WorkerShutdownRequest<'a>(&'a SearchWorker);

impl Drop for WorkerShutdownRequest<'_> {
    fn drop(&mut self) {
        self.0.request_shutdown();
    }
}

impl HelpBrowser {
    fn new(topics: Arc<[HelpTopic]>, library_paths: Vec<String>, query: &str) -> Self {
        let initial_results = if query.is_empty() {
            (0..topics.len().min(MAX_FILTERED_RESULTS))
                .map(|index| (index, 0))
                .collect()
        } else {
            Vec::new()
        };
        HelpBrowser {
            library_paths,
            topics,
            query: query.to_string(),
            cursor_pos: query.chars().count(),
            filtered: initial_results,
            pending_generation: None,
            pending_open: None,
            query_generation: 0,
            search_dirty: false,
            selected: 0,
            scroll_offset: 0,
            text_scroll: TextScrollState::new(),
        }
    }

    fn reset_results(&mut self) {
        self.selected = 0;
        self.scroll_offset = 0;
        self.text_scroll = TextScrollState::new();
        // Ensure cursor_pos stays within bounds
        let query_len = self.query.chars().count();
        if self.cursor_pos > query_len {
            self.cursor_pos = query_len;
        }
    }

    fn run(&mut self) -> io::Result<()> {
        let mut worker = SearchWorker::spawn(Arc::clone(&self.topics))?;
        if !self.query.is_empty() {
            self.query_generation = self.query_generation.wrapping_add(1);
            self.pending_generation = Some(worker.submit(self.query.clone())?);
        }
        let result = with_alternate_screen(|| {
            let _shutdown = WorkerShutdownRequest(&worker);
            self.run_inner(&worker)
        });
        worker.request_shutdown();
        let shutdown = worker.shutdown_and_join();
        result.and(shutdown)
    }

    fn run_inner(&mut self, worker: &SearchWorker) -> io::Result<()> {
        let mut stdout = io::stdout();
        let poll_timeout = Duration::from_millis(16);
        let mut needs_redraw = true;
        loop {
            if self.update_text_scroll() {
                needs_redraw = true;
            }
            if needs_redraw {
                self.render(&mut stdout)?;
                needs_redraw = false;
            }
            let timeout = if self.search_pending() {
                poll_timeout
            } else {
                Duration::from_millis(50)
            };
            let mut first_poll = true;
            let drained = drain_help_events(
                || {
                    let event_timeout = if first_poll {
                        first_poll = false;
                        timeout
                    } else {
                        Duration::ZERO
                    };
                    if !event::poll(event_timeout)? {
                        return Ok(None);
                    }
                    let event = event::read()?;
                    log::debug!("help_browser: received event: {:?}", event);
                    Ok(Some(event))
                },
                |event| {
                    let too_small = check_terminal_too_small(&MIN_SIZE).is_some();
                    self.handle_event(event, too_small, worker)
                },
            )?;
            needs_redraw |= drained.redraw;
            match drained.action {
                BrowserAction::Exit => break,
                BrowserAction::Open(index) => {
                    if self.search_dirty {
                        self.dispatch_search(worker)?;
                    }
                    self.open_topic(index);
                    needs_redraw = true;
                }
                BrowserAction::Continue => {
                    if self.search_dirty {
                        self.dispatch_search(worker)?;
                    }
                }
            }
            if needs_redraw {
                self.render(&mut stdout)?;
                needs_redraw = false;
            }
            let backlog = poll_backlog_before_results(
                drained.action,
                || event::poll(Duration::ZERO),
                || {
                    if let Some(generation) = self.pending_generation
                        && let Some(result) = worker.take_result(generation)?
                        && self.pending_generation == Some(result.generation)
                    {
                        let open_index = self.accept_search_result(result);
                        needs_redraw = true;
                        if let Some(index) = open_index {
                            self.open_topic(index);
                        }
                    }
                    Ok(())
                },
            )?;
            if backlog || matches!(drained.action, BrowserAction::Open(_)) {
                continue;
            }
        }
        Ok(())
    }

    fn start_search(&mut self) {
        self.pending_open = None;
        self.query_generation = self.query_generation.wrapping_add(1);
        self.pending_generation = None;
        self.search_dirty = true;
        if self.query.is_empty() {
            self.reset_results();
            self.filtered = (0..self.topics.len().min(MAX_FILTERED_RESULTS))
                .map(|index| (index, 0))
                .collect();
        }
    }

    fn dispatch_search(&mut self, worker: &SearchWorker) -> io::Result<()> {
        self.search_dirty = false;
        if self.query.is_empty() {
            self.pending_generation = None;
            worker.cancel()?;
        } else {
            self.pending_generation = Some(worker.submit(self.query.clone())?);
        }
        Ok(())
    }

    fn search_pending(&self) -> bool {
        !self.query.is_empty() && (self.search_dirty || self.pending_generation.is_some())
    }

    fn insert_query_char(&mut self, character: char) {
        let byte_pos = self
            .query
            .char_indices()
            .nth(self.cursor_pos)
            .map(|(index, _)| index)
            .unwrap_or(self.query.len());
        self.query.insert(byte_pos, character);
        self.cursor_pos += 1;
        self.start_search();
    }

    fn backspace_query(&mut self) {
        self.pending_open = None;
        if self.cursor_pos > 0 {
            let byte_pos = self
                .query
                .char_indices()
                .nth(self.cursor_pos - 1)
                .map(|(index, _)| index)
                .unwrap_or(0);
            self.query.remove(byte_pos);
            self.cursor_pos -= 1;
            self.start_search();
        }
    }

    fn delete_query_char(&mut self) {
        self.pending_open = None;
        if self.cursor_pos < self.query.chars().count() {
            let byte_pos = self
                .query
                .char_indices()
                .nth(self.cursor_pos)
                .map(|(index, _)| index)
                .unwrap_or(self.query.len());
            self.query.remove(byte_pos);
            self.start_search();
        }
    }

    fn clear_query(&mut self) {
        self.query.clear();
        self.cursor_pos = 0;
        self.start_search();
    }

    fn move_cursor(&mut self, cursor_pos: usize) {
        self.pending_open = None;
        self.cursor_pos = cursor_pos.min(self.query.chars().count());
    }

    fn move_selection(&mut self, direction: isize, visible_rows: usize) {
        self.pending_open = None;
        if self.search_pending() {
            return;
        }
        if direction < 0 && self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
        } else if direction > 0 && self.selected + 1 < self.filtered.len() {
            self.selected += 1;
            if self.selected >= self.scroll_offset + visible_rows {
                self.scroll_offset = self.selected - visible_rows + 1;
            }
        }
    }

    fn cancel_search(&mut self, worker: &SearchWorker) -> io::Result<()> {
        self.clear_pending_search();
        worker.cancel()?;
        Ok(())
    }

    fn remember_pending_open(&mut self) {
        self.pending_open = self.search_pending().then_some(self.query_generation);
    }

    fn clear_pending_search(&mut self) {
        if self.pending_generation.take().is_some() {
            self.filtered.clear();
            self.reset_results();
        }
        self.pending_open = None;
        self.search_dirty = false;
    }

    fn accept_search_result(&mut self, result: search::SearchResult) -> Option<usize> {
        if self.pending_generation != Some(result.generation) {
            return None;
        }
        self.filtered = result.matches;
        self.pending_generation = None;
        self.reset_results();
        if self.pending_open.take() == Some(self.query_generation) {
            self.filtered.first().map(|(index, _)| *index)
        } else {
            self.pending_open = None;
            None
        }
    }

    fn open_topic(&mut self, index: usize) {
        let Some(topic) = self.topics.get(index) else {
            return;
        };
        let title = topic.qualified_name();
        match topic.entry_type.as_str() {
            "vignette" => {
                let content = get_vignette_text(&topic.topic, &topic.package)
                    .unwrap_or_else(|error| format!("{error}"));
                if let Err(error) = display_help_pager(&title, &content, false) {
                    log::error!("help_browser: pager error: {error}");
                }
            }
            "demo" => {
                let content = format!(
                    r#"This is a demo entry.

To run the demo, execute in R:

demo("{name}", package = "{pkg}")"#,
                    name = topic.topic,
                    pkg = topic.package,
                );
                if let Err(error) = display_help_pager(&title, &content, false) {
                    log::error!("help_browser: pager error: {error}");
                }
            }
            _ => {
                let result = HelpTargetResolver::prepare_candidate(topic)
                    .map_err(io::Error::other)
                    .and_then(|page| {
                        display_help_pages(vec![page], self.library_paths.clone(), false)
                    });
                if let Err(error) = result {
                    let message = help_page_load_error_message(&error);
                    if let Err(pager_error) = display_help_pager(&title, &message, false) {
                        log::error!("help_browser: failed to display help error: {pager_error}");
                    }
                }
            }
        }
    }
}

pub fn run_help_browser(query: &str) -> io::Result<()> {
    let libraries =
        match help_library_paths_after_refresh(refresh_lib_paths_from_r(), cached_lib_paths) {
            Ok(paths) => paths,
            Err(error) => {
                println!("# Error loading help database: {error}");
                return Ok(());
            }
        };
    let topics = get_help_topics_from_paths(&libraries);
    if topics.is_empty() {
        println!("# No help topics found. Make sure R packages are installed.");
        return Ok(());
    }
    HelpBrowser::new(topics.into(), libraries, query).run()
}
