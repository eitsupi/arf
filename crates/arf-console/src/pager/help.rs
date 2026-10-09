//! Interactive fuzzy help search for R documentation.
//!
//! This module provides a terminal-based fuzzy search interface for R help topics
//! loaded from installed packages' `Meta/Rd.rds`, `Meta/vignette.rds`, and
//! `Meta/demo.rds` independently.
//!
//! # Acknowledgment
//!
//! This implementation is inspired by the **felp** package by Atsushi Yasumoto (atusy):
//! - Repository: <https://github.com/atusy/felp>
//! - CRAN: <https://cran.r-project.org/package=felp>
//!
//! The concept of fuzzy help search was learned from felp's `fuzzyhelp()` function;
//! help indexes are read directly from installed packages here.

use super::text_utils::{
    display_width, exceeds_width, pad_to_width, scroll_display, truncate_to_width,
};
use super::{
    MinimumSize, TextScrollState, check_terminal_too_small, render_size_warning,
    with_alternate_screen,
};
#[cfg(test)]
use crate::fuzzy::fuzzy_match_with_case_preference;
use arf_harp::HarpResult;
use arf_harp::help::{
    HelpTargetResolver, HelpTopic, get_help_topics_from_paths, get_vignette_text,
};
use arf_harp::help_bridge::{PreparedHelpPage, PreparedHelpRequest};
use arf_harp::lib_paths::{cached_lib_paths, refresh_lib_paths_from_r};
use crossterm::{
    ExecutableCommand, cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind},
    queue,
    style::Stylize,
    terminal::{self, BeginSynchronizedUpdate, EndSynchronizedUpdate},
};
use std::io::{self, Write};
use std::sync::Arc;
use std::time::Duration;

mod search;
#[cfg(test)]
use search::SearchBench;
use search::SearchWorker;
#[cfg(test)]
use search::search_topics_sync;

/// Maximum number of results to keep in filtered list.
const MAX_FILTERED_RESULTS: usize = 500;

/// Minimum terminal size for the help browser.
///
/// Width: prefix(3) + name_min(20) + spacing(1) + some title room = ~30 columns.
/// Height: 5 lines of chrome + 3 minimum content rows = 8.
const MIN_SIZE: MinimumSize = MinimumSize { cols: 30, rows: 8 };

/// Run the interactive help browser.
///
/// If `query` is non-empty, the browser opens with the query pre-filled,
/// allowing the user to refine and select a topic.
///
/// Returns `Ok(())` when the user exits the browser (Esc, Ctrl+C, or Ctrl+D),
/// or an error if something goes wrong.
pub fn run_help_browser(query: &str) -> io::Result<()> {
    // Refresh once before metadata discovery, then keep this snapshot for the
    // browser and any viewer opened from it.
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

    let mut browser = HelpBrowser::new(topics.into(), libraries, query);
    browser.run()
}

fn help_library_paths_after_refresh(
    refresh: HarpResult<Vec<String>>,
    cached: impl FnOnce() -> Vec<String>,
) -> HarpResult<Vec<String>> {
    match refresh {
        Ok(paths) => Ok(paths),
        Err(error) => {
            let paths = cached();
            if paths.is_empty() {
                return Err(error);
            }
            log::warn!(
                "Could not refresh R library paths for help; using the previous snapshot: {error}"
            );
            Ok(paths)
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BrowserAction {
    Continue,
    Open(usize),
    Exit,
}

struct EventHandling {
    action: BrowserAction,
    redraw: bool,
}

struct EventDrain {
    action: BrowserAction,
    events_read: usize,
    redraw: bool,
}

fn drain_help_events(
    mut next_event: impl FnMut() -> io::Result<Option<Event>>,
    mut handle_event: impl FnMut(Event) -> io::Result<EventHandling>,
) -> io::Result<EventDrain> {
    let mut drained = EventDrain {
        action: BrowserAction::Continue,
        events_read: 0,
        redraw: false,
    };
    while drained.events_read < 32 {
        let Some(event) = next_event()? else {
            break;
        };
        drained.events_read += 1;
        let handling = handle_event(event)?;
        drained.redraw |= handling.redraw;
        drained.action = handling.action;
        if handling.action != BrowserAction::Continue {
            break;
        }
    }
    Ok(drained)
}

fn poll_backlog_before_results(
    action: BrowserAction,
    poll_input: impl FnOnce() -> io::Result<bool>,
    apply_results: impl FnOnce() -> io::Result<()>,
) -> io::Result<bool> {
    if poll_input()? {
        return Ok(true);
    }
    if action == BrowserAction::Continue {
        apply_results()?;
    }
    Ok(false)
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

    fn handle_event(
        &mut self,
        event: Event,
        too_small: bool,
        worker: &SearchWorker,
    ) -> io::Result<EventHandling> {
        let continue_with = |redraw| EventHandling {
            action: BrowserAction::Continue,
            redraw,
        };
        match event {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return Ok(continue_with(false));
                }
                if too_small {
                    match (key.code, key.modifiers) {
                        (KeyCode::Esc, _)
                        | (KeyCode::Char('q'), KeyModifiers::NONE)
                        | (KeyCode::Char('c'), KeyModifiers::CONTROL)
                        | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                            self.cancel_search(worker)?;
                            return Ok(EventHandling {
                                action: BrowserAction::Exit,
                                redraw: true,
                            });
                        }
                        _ => return Ok(continue_with(true)),
                    }
                }

                match (key.code, key.modifiers) {
                    // Exit
                    (KeyCode::Esc, _)
                    | (KeyCode::Char('c'), KeyModifiers::CONTROL)
                    | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                        self.cancel_search(worker)?;
                        return Ok(EventHandling {
                            action: BrowserAction::Exit,
                            redraw: true,
                        });
                    }

                    (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                        self.move_selection(-1, visible_result_rows());
                    }
                    (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                        self.move_selection(1, visible_result_rows());
                    }

                    (KeyCode::Enter, _) | (KeyCode::Tab, _) => {
                        if self.search_pending() {
                            self.remember_pending_open();
                        } else if let Some(&(index, _)) = self.filtered.get(self.selected) {
                            return Ok(EventHandling {
                                action: BrowserAction::Open(index),
                                redraw: true,
                            });
                        }
                    }

                    (KeyCode::Backspace, _) => {
                        self.backspace_query();
                    }
                    (KeyCode::Delete, _) => {
                        self.delete_query_char();
                    }
                    (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                        self.clear_query();
                    }
                    (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                        self.insert_query_char(c);
                    }
                    (KeyCode::Left, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                        self.move_cursor(self.cursor_pos.saturating_sub(1));
                    }
                    (KeyCode::Right, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                        self.move_cursor((self.cursor_pos + 1).min(self.query.chars().count()));
                    }
                    (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                        self.move_cursor(0);
                    }
                    (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                        self.move_cursor(self.query.chars().count());
                    }

                    _ => {}
                }
                Ok(continue_with(true))
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::ScrollUp => {
                    self.move_selection(-1, visible_result_rows());
                    Ok(continue_with(true))
                }
                MouseEventKind::ScrollDown => {
                    self.move_selection(1, visible_result_rows());
                    Ok(continue_with(true))
                }
                _ => Ok(continue_with(false)),
            },
            Event::Resize(_, _) => Ok(continue_with(true)),
            _ => Ok(continue_with(false)),
        }
    }

    fn start_search(&mut self) {
        self.pending_open = None;
        self.query_generation = self.query_generation.wrapping_add(1);
        self.pending_generation = None;
        self.search_dirty = true;
        self.reset_results();
        if self.query.is_empty() {
            self.filtered = (0..self.topics.len().min(MAX_FILTERED_RESULTS))
                .map(|index| (index, 0))
                .collect();
        } else {
            self.filtered.clear();
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

    /// Update the text scroll animation state.
    fn update_text_scroll(&mut self) -> bool {
        self.text_scroll.update(self.selected)
    }

    fn render(&self, stdout: &mut io::Stdout) -> io::Result<()> {
        if let Some((cols, rows)) = check_terminal_too_small(&MIN_SIZE) {
            return render_size_warning(stdout, cols, rows, &MIN_SIZE);
        }
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        self.render_to(stdout, cols as usize, rows as usize)
    }

    fn render_to<W: Write>(&self, stdout: &mut W, width: usize, rows: usize) -> io::Result<()> {
        // Begin synchronized update to prevent flickering
        queue!(stdout, BeginSynchronizedUpdate)?;

        // Move cursor to top-left and hide it
        stdout.execute(cursor::MoveTo(0, 0))?;
        stdout.execute(cursor::Hide)?;

        // Header
        let status = if self.search_pending() {
            "Searching…".to_string()
        } else {
            format!("{} topics", self.filtered.len())
        };
        let header = format!("─ Help Search [{status}] ─");
        let padded_header = format!("{:─<width$}", header, width = width);
        writeln!(stdout, "\r{}", padded_header.dark_grey())?;

        // Query input with cursor at correct position
        let before_cursor: String = self.query.chars().take(self.cursor_pos).collect();
        let after_cursor: String = self.query.chars().skip(self.cursor_pos).collect();
        let query_line = format!("  Filter: {}_{}", before_cursor, after_cursor);
        writeln!(stdout, "\r{}", pad_to_width(&query_line, width))?;

        // Separator
        writeln!(stdout, "\r{}", "─".repeat(width).dark_grey())?;

        // Results
        let (name_width, title_width) = calculate_layout(width);
        let visible_rows = visible_result_rows_for(rows);

        for i in 0..visible_rows {
            let idx = self.scroll_offset + i;
            if self.search_pending() {
                let line = if i == 0 { "  Searching…" } else { "" };
                writeln!(stdout, "\r{}", pad_to_width(line, width).dark_grey())?;
            } else if idx < self.filtered.len() {
                let (topic_index, _score) = self.filtered[idx];
                let topic = &self.topics[topic_index];
                let prefix = if idx == self.selected { " > " } else { "   " };
                let name = topic.qualified_name();

                // For selected item, use scrolling display if text is truncated
                let (display_name, display_title) = if idx == self.selected {
                    let name_truncated = exceeds_width(&name, name_width);
                    let title_truncated = exceeds_width(&topic.title, title_width);

                    if name_truncated || title_truncated {
                        let (scrolled_name, _) =
                            scroll_display(&name, name_width, self.text_scroll.scroll_pos);
                        let (scrolled_title, _) =
                            scroll_display(&topic.title, title_width, self.text_scroll.scroll_pos);
                        (scrolled_name, scrolled_title)
                    } else {
                        (name.clone(), topic.title.clone())
                    }
                } else {
                    (
                        truncate_to_width(&name, name_width),
                        truncate_to_width(&topic.title, title_width),
                    )
                };

                // Build line with display-width-aware padding
                let padded_name = pad_to_width(&display_name, name_width);
                let content = format!("{}{} {}", prefix, padded_name, display_title);
                let line = pad_to_width(&content, width);

                if idx == self.selected {
                    writeln!(stdout, "\r{}", line.reverse())?;
                } else {
                    // Apply dark_grey only to the title portion for non-selected items
                    let name_part = format!("{}{} ", prefix, padded_name);
                    let title_part = truncate_to_width(
                        &display_title,
                        width.saturating_sub(display_width(&name_part)),
                    );
                    let padding_len = width
                        .saturating_sub(display_width(&name_part) + display_width(&title_part));
                    writeln!(
                        stdout,
                        "\r{}{}{}",
                        name_part,
                        title_part.dark_grey(),
                        " ".repeat(padding_len)
                    )?;
                }
            } else {
                writeln!(stdout, "\r{}", " ".repeat(width))?;
            }
        }

        // Footer
        writeln!(stdout, "\r{}", "─".repeat(width).dark_grey())?;

        // Build plain text first, pad it, then apply style.
        // pad_to_width is not ANSI-aware, so styling must come after padding.
        let footer_plain = "  ↑↓ navigate Tab/Enter select Esc exit";
        writeln!(
            stdout,
            "\r{}",
            pad_to_width(footer_plain, width).dark_grey()
        )?;

        // End synchronized update
        queue!(stdout, EndSynchronizedUpdate)?;
        stdout.flush()?;
        Ok(())
    }
}

struct WorkerShutdownRequest<'a>(&'a SearchWorker);

impl Drop for WorkerShutdownRequest<'_> {
    fn drop(&mut self) {
        self.0.request_shutdown();
    }
}

#[cfg(test)]
fn fuzzy_search_topics(topics: &[HelpTopic], query: &str) -> Vec<(HelpTopic, u32)> {
    search_topics_sync(topics, query)
        .into_iter()
        .map(|(index, score)| (topics[index].clone(), score))
        .collect()
}

/// Calculate layout widths for the help browser display.
/// Returns (name_width, title_width) based on terminal columns.
fn calculate_layout(cols: usize) -> (usize, usize) {
    let prefix_width = 3; // " > " or "   "
    let spacing = 1; // space between name and title
    let name_width = (cols / 3).max(20); // ~1/3 of screen for name, min 20
    let title_width = cols.saturating_sub(prefix_width + name_width + spacing + 1);
    (name_width, title_width)
}

/// Calculate the number of visible result rows based on terminal height.
/// Layout: header(1) + filter(1) + separator(1) + results(N) + separator(1) + footer(1)
fn visible_result_rows() -> usize {
    let (_, rows) = terminal::size().unwrap_or((80, 24));
    visible_result_rows_for(rows as usize)
}

fn visible_result_rows_for(rows: usize) -> usize {
    // Reserve 5 lines for UI chrome (header, filter, 2 separators, footer)
    rows.saturating_sub(5).max(3)
}

/// Display help content (Markdown) in an interactive pager.
///
/// Content is rendered from Markdown to styled ratatui lines using
/// `pulldown-cmark`. Both help topics (via `rd2qmd`) and vignettes
/// (via `r-vignette-to-md`) produce Markdown, so this is the unified
/// rendering path.
///
/// Plain text (demo messages, error messages) also renders fine since
/// it contains no Markdown syntax.
fn display_help_pager(title: &str, content: &str, manage_alternate_screen: bool) -> io::Result<()> {
    use super::help_content::HelpContent;
    use super::{PagerConfig, run};

    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let mut content = HelpContent::new(content, cols as usize, rows as usize);

    let config = PagerConfig {
        title,
        footer_hint: "↑↓/jk scroll  / search  q/Esc back",
        manage_alternate_screen,
    };

    run(&mut content, &config)
}

/// Display a prepared help request without rereading the compiled help database.
///
/// Multiple pages require an explicit selector confirmation. Exiting the
/// selector without confirming is a normal cancellation and displays nothing.
pub(crate) fn display_prepared_help_request(request: &PreparedHelpRequest) -> io::Result<()> {
    if request.pages.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "prepared help request has no pages",
        ));
    }
    // R selected and prepared these exact pages already. The cached snapshot
    // only controls additional cross-package navigation and may be empty.
    display_help_pages(request.pages.clone(), cached_lib_paths(), true)
}

fn display_help_pages(
    pages: Vec<PreparedHelpPage>,
    libraries: Vec<String>,
    manage_alternate_screen: bool,
) -> io::Result<()> {
    use super::help_session::HelpViewer;
    use super::{PagerConfig, run};

    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let mut viewer = HelpViewer::new(pages, libraries, cols as usize, rows as usize);
    let config = PagerConfig {
        manage_alternate_screen,
        ..PagerConfig::default()
    };
    run(&mut viewer, &config)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HelpPageSelectorState {
    page_count: usize,
    pub(super) selected: Option<usize>,
    confirmed: Option<usize>,
}

impl HelpPageSelectorState {
    pub(super) fn new(page_count: usize) -> Self {
        Self {
            page_count,
            selected: None,
            confirmed: None,
        }
    }

    pub(super) fn move_up(&mut self) -> bool {
        let Some(selected) = self.selected else {
            self.selected = self.page_count.checked_sub(1);
            return self.selected.is_some();
        };
        if selected == 0 {
            self.selected = None;
            return true;
        }
        self.selected = Some(selected - 1);
        true
    }

    /// Returns `Some(true)` when this selects the first row from the empty
    /// state; the pager must not scroll its viewport for that movement.
    pub(super) fn move_down(&mut self) -> Option<bool> {
        match self.selected {
            None if self.page_count > 0 => {
                self.selected = Some(0);
                Some(true)
            }
            Some(selected) if selected + 1 < self.page_count => {
                self.selected = Some(selected + 1);
                Some(false)
            }
            _ => None,
        }
    }

    pub(super) fn confirm(&mut self) -> Option<usize> {
        let selected = self.selected?;
        self.confirmed = Some(selected);
        Some(selected)
    }
}

pub(super) fn help_page_title(package: &str, display_topic: &str) -> String {
    format!("{package}::{display_topic}")
}

fn help_page_load_error_message(error: &io::Error) -> String {
    format!("Unable to load this help topic.\n\n{error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic(package: &str, name: &str, aliases: &[&str], title: &str) -> HelpTopic {
        HelpTopic {
            package: package.to_string(),
            package_dir: std::path::PathBuf::from(format!("/{package}")),
            topic: name.to_string(),
            aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
            help_key: None,
            title: title.to_string(),
            entry_type: "help".to_string(),
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
            browser.accept_search_result(search::SearchResult {
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
    fn candidate_movement_keeps_pending_search_and_only_moves_ready_candidates() {
        let topics: Arc<[HelpTopic]> = vec![
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

        browser.accept_search_result(search::SearchResult {
            generation: 12,
            matches: vec![(0, 8), (1, 4)],
        });
        browser.move_selection(1, 5);
        assert_eq!(browser.selected, 1);
    }

    #[test]
    fn ready_enter_stops_event_drain_before_later_browser_keys() {
        use crossterm::event::KeyEvent;
        use std::collections::VecDeque;

        let topics: Arc<[HelpTopic]> = vec![topic("base", "mean", &[], "Mean")].into();
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
        use crossterm::event::KeyEvent;
        use std::collections::VecDeque;

        let topics: Arc<[HelpTopic]> = vec![topic("base", "mean", &[], "Mean")].into();
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
        assert!(!applied, "queued input takes priority over pending open");

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
        use crossterm::event::KeyEvent;
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
        use crossterm::event::KeyEvent;
        use std::collections::VecDeque;

        let topics: Arc<[HelpTopic]> = vec![topic("base", "mean", &[], "Mean")].into();
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

    #[test]
    fn stale_empty_result_cannot_open_and_query_edit_clears_pending_enter() {
        let topics: Arc<[HelpTopic]> = vec![topic("base", "mean", &[], "Mean")].into();
        let mut browser = HelpBrowser::new(Arc::clone(&topics), Vec::new(), "missing");
        browser.query_generation = 9;
        browser.pending_generation = Some(9);
        browser.remember_pending_open();

        assert_eq!(
            browser.accept_search_result(search::SearchResult {
                generation: 8,
                matches: vec![(0, 1)],
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
            browser.accept_search_result(search::SearchResult {
                generation: 10,
                matches: Vec::new(),
            }),
            None
        );
        assert!(browser.filtered.is_empty());
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
        let refreshed = help_library_paths_after_refresh(Ok(vec!["library-a".into()]), || {
            cached_read.set(true);
            vec!["library-b".into()]
        })
        .unwrap();
        assert_eq!(refreshed, ["library-a"]);
        assert!(
            !cached_read.get(),
            "successful refresh should be authoritative"
        );

        let previous = help_library_paths_after_refresh(
            Err(arf_harp::HarpError::TypeMismatch {
                expected: "library path snapshot".into(),
                actual: "refresh failed".into(),
            }),
            || vec!["library-b".into()],
        )
        .unwrap();
        assert_eq!(previous, ["library-b"]);

        let error = help_library_paths_after_refresh(
            Err(arf_harp::HarpError::TypeMismatch {
                expected: "library path snapshot".into(),
                actual: "refresh failed".into(),
            }),
            Vec::new,
        )
        .expect_err("an empty cache cannot support :help metadata discovery");
        match error {
            arf_harp::HarpError::TypeMismatch { expected, actual } => {
                assert_eq!(expected, "library path snapshot");
                assert_eq!(actual, "refresh failed");
            }
            other => panic!("refresh error should retain its type: {other}"),
        }
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
        use crate::pager::PagerContent;
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

        let topics = get_help_topics_from_paths(&snapshot);
        let topics: Arc<[HelpTopic]> = topics.into();
        let mut browser = HelpBrowser::new(Arc::clone(&topics), snapshot, "resolverpkg");
        browser.filtered = search_topics_sync(&topics, "resolverpkg");
        let selected_index = browser
            .filtered
            .iter()
            .position(|(index, _)| {
                let topic = &topics[*index];
                topic.package == "resolverpkg"
                    && topic.package_dir == source_a
                    && topic.help_key.is_some()
            })
            .expect("the first installed package copy should appear in metadata results");
        browser.selected = selected_index;
        let topic = &topics[browser.filtered[browser.selected].0];
        let mut page = HelpTargetResolver::prepare_candidate(topic).unwrap();
        assert_eq!(page.package_dir, source_a);
        // Add a deterministic qualified link to the real prepared fixture page.
        page.markdown = "[target](x-r-help:targetpkg/lm)".to_owned();

        let mut viewer = super::super::help_session::HelpViewer::new(
            vec![page],
            browser.library_paths.clone(),
            40,
            8,
        );
        assert_eq!(viewer.title(), Some("resolverpkg::mean"));
        assert_eq!(
            PagerContent::handle_key(&mut viewer, KeyCode::Tab, KeyModifiers::NONE),
            Some(super::super::PagerAction::Redraw)
        );
        assert!(matches!(
            PagerContent::handle_key(&mut viewer, KeyCode::Enter, KeyModifiers::NONE),
            Some(super::super::PagerAction::ScrollTo(0))
        ));
        assert_eq!(viewer.title(), Some("targetpkg::lm"));
        assert_ne!(target_a, target_b);
        assert!(
            !PagerContent::feedback_message(&viewer)
                .unwrap_or_default()
                .contains("Unable to open")
        );
    }

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
        // Japanese characters (each is 2 display columns)
        // "日本語テスト" = 12 cols, max 7 → "日本語…" (6+1=7)
        assert_eq!(truncate_to_width("日本語テスト", 7), "日本語…");
        assert_eq!(truncate_to_width("日本語", 10), "日本語");
    }

    #[test]
    fn test_calculate_layout_standard() {
        // 80 columns: name_width = 80/3 = 26, title_width = 80 - 3 - 26 - 1 - 1 = 49
        let (name_width, title_width) = calculate_layout(80);
        assert_eq!(name_width, 26);
        assert_eq!(title_width, 49);
    }

    #[test]
    fn test_calculate_layout_wide() {
        // 120 columns: name_width = 120/3 = 40, title_width = 120 - 3 - 40 - 1 - 1 = 75
        let (name_width, title_width) = calculate_layout(120);
        assert_eq!(name_width, 40);
        assert_eq!(title_width, 75);
    }

    #[test]
    fn test_calculate_layout_narrow() {
        // 60 columns: name_width = max(60/3, 20) = 20, title_width = 60 - 3 - 20 - 1 - 1 = 35
        let (name_width, title_width) = calculate_layout(60);
        assert_eq!(name_width, 20);
        assert_eq!(title_width, 35);
    }

    #[test]
    fn test_calculate_layout_very_narrow() {
        // 40 columns: name_width = max(40/3, 20) = 20, title_width = 40 - 3 - 20 - 1 - 1 = 15
        let (name_width, title_width) = calculate_layout(40);
        assert_eq!(name_width, 20);
        assert_eq!(title_width, 15);
    }

    #[test]
    fn test_fuzzy_search_topics() {
        let topics = vec![
            HelpTopic {
                package: "base".to_string(),
                package_dir: std::path::PathBuf::from("/base"),
                topic: "print".to_string(),
                aliases: vec!["print.default".to_string()],
                help_key: Some("print".to_string()),
                title: "Print Values".to_string(),
                entry_type: "help".to_string(),
            },
            HelpTopic {
                package: "dplyr".to_string(),
                package_dir: std::path::PathBuf::from("/dplyr"),
                topic: "mutate".to_string(),
                aliases: vec![],
                help_key: None,
                title: "Create, modify, and delete columns".to_string(),
                entry_type: "help".to_string(),
            },
        ];

        let results = fuzzy_search_topics(&topics, "print");
        assert!(!results.is_empty());
        assert_eq!(results[0].0.topic, "print");

        let results = fuzzy_search_topics(&topics, "mut");
        assert!(!results.is_empty());
        assert_eq!(results[0].0.topic, "mutate");
    }

    #[test]
    fn fuzzy_search_prefers_matching_case_without_filtering_variants() {
        let topics = vec![
            HelpTopic {
                package: "base".to_string(),
                package_dir: std::path::PathBuf::from("/base"),
                topic: "foo_bar".to_string(),
                aliases: vec![],
                help_key: None,
                title: "Lowercase topic".to_string(),
                entry_type: "help".to_string(),
            },
            HelpTopic {
                package: "base".to_string(),
                package_dir: std::path::PathBuf::from("/base"),
                topic: "Foo".to_string(),
                aliases: vec![],
                help_key: None,
                title: "Capitalized topic".to_string(),
                entry_type: "help".to_string(),
            },
        ];

        let lowercase_results = fuzzy_search_topics(&topics, "foo");
        assert!(
            lowercase_results
                .iter()
                .any(|(topic, _)| topic.topic == "foo_bar")
        );
        assert!(
            lowercase_results
                .iter()
                .any(|(topic, _)| topic.topic == "Foo")
        );

        let uppercase_results = fuzzy_search_topics(&topics, "Foo");
        assert_eq!(uppercase_results.len(), 2);
        assert_eq!(uppercase_results[0].0.topic, "Foo");
        assert_eq!(uppercase_results[1].0.topic, "foo_bar");
    }

    #[test]
    fn test_fuzzy_search_topics_empty_query() {
        let topics = vec![HelpTopic {
            package: "base".to_string(),
            package_dir: std::path::PathBuf::from("/base"),
            topic: "print".to_string(),
            aliases: vec![],
            help_key: None,
            title: "Print Values".to_string(),
            entry_type: "help".to_string(),
        }];

        // Empty query should match everything
        let results = fuzzy_search_topics(&topics, "");
        assert!(!results.is_empty());
    }

    #[test]
    fn test_fuzzy_search_topics_no_match() {
        let topics = vec![HelpTopic {
            package: "base".to_string(),
            package_dir: std::path::PathBuf::from("/base"),
            topic: "print".to_string(),
            aliases: vec![],
            help_key: None,
            title: "Print Values".to_string(),
            entry_type: "help".to_string(),
        }];

        let results = fuzzy_search_topics(&topics, "xyz123");
        assert!(results.is_empty());
    }

    #[test]
    fn fuzzy_search_matches_aliases_without_duplicating_topic_results() {
        let topic = HelpTopic {
            package: "base".to_string(),
            package_dir: std::path::PathBuf::from("/base"),
            topic: "print".to_string(),
            aliases: vec!["print.default".to_string(), "print.value".to_string()],
            help_key: Some("print".to_string()),
            title: "Print Values".to_string(),
            entry_type: "help".to_string(),
        };

        let results = fuzzy_search_topics(std::slice::from_ref(&topic), "print.value");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.topic, "print");

        let results = fuzzy_search_topics(&[topic], "base::print.value");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.qualified_name(), "base::print");
    }

    #[test]
    fn fuzzy_search_matches_help_key_bare_and_qualified_without_changing_display_topic() {
        let topic = HelpTopic {
            package: "base".to_string(),
            package_dir: std::path::PathBuf::from("/base"),
            topic: "[.data.frame".to_string(),
            aliases: vec!["[.data.frame".to_string()],
            help_key: Some("Extract.data.frame".to_string()),
            title: "Extract or Replace Parts of an Object".to_string(),
            entry_type: "help".to_string(),
        };

        for query in ["Extract.data.frame", "base::Extract.data.frame"] {
            let results = fuzzy_search_topics(std::slice::from_ref(&topic), query);
            assert_eq!(results.len(), 1, "query {query:?}");
            assert_eq!(results[0].0.topic, "[.data.frame");
        }
    }

    #[test]
    fn fuzzy_search_keeps_source_order_for_equal_ranks() {
        let topics = [
            topic("first", "needle", &[], ""),
            topic("second", "needle", &[], ""),
        ];
        let results = fuzzy_search_topics(&topics, "needle");
        assert_eq!(
            results
                .iter()
                .map(|(topic, _)| topic.package.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn fuzzy_search_weights_titles_by_half() {
        let title_only = topic("pkg", "other", &[], "Needle");
        let results = fuzzy_search_topics(std::slice::from_ref(&title_only), "Needle");
        assert_eq!(results.len(), 1);
        let raw_score = crate::fuzzy::fuzzy_match_smart_case("Needle", "Needle")
            .unwrap()
            .score;
        assert_eq!(results[0].1, raw_score / 2);
    }

    #[test]
    fn fuzzy_search_prefers_smart_title_over_ignore_case_bare_name() {
        let topics = [
            topic("pkg", "unrelated", &[], "Foo"),
            topic("pkg", "foo", &[], ""),
        ];
        let results = fuzzy_search_topics(&topics, "Foo");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0.topic, "unrelated");
        assert_eq!(results[1].0.topic, "foo");
    }

    #[test]
    fn fuzzy_search_keeps_dollar_literal_and_matches_unicode() {
        let topics = [
            topic("base", "Extract$", &[], ""),
            topic("base", "Extract", &[], ""),
            topic("unicode", "café_日本語", &[], ""),
        ];
        let literal_dollar_matches = fuzzy_search_topics(&topics, "Extract$");
        assert_eq!(literal_dollar_matches.len(), 1);
        assert_eq!(literal_dollar_matches[0].0.topic, "Extract$");
        assert_eq!(
            fuzzy_search_topics(&topics, "cafe\u{301}日本語")[0].0.topic,
            "café_日本語"
        );
    }

    #[test]
    fn fuzzy_search_does_not_join_separate_aliases() {
        let item = topic("pkg", "other", &["foo", "bar"], "");
        for query in ["fb", "f b"] {
            assert!(
                fuzzy_search_topics(std::slice::from_ref(&item), query).is_empty(),
                "query {query:?} must not span aliases"
            );
        }
    }

    #[test]
    fn fuzzy_search_limits_before_cloning_and_empty_search_keeps_first_topics() {
        let topics: Vec<_> = (0..MAX_FILTERED_RESULTS + 25)
            .map(|index| topic("pkg", &format!("same{index}"), &[], ""))
            .collect();

        let empty = fuzzy_search_topics(&topics, "");
        assert_eq!(empty.len(), MAX_FILTERED_RESULTS);
        assert_eq!(empty[0].0.topic, "same0");
        assert_eq!(empty[MAX_FILTERED_RESULTS - 1].0.topic, "same499");

        let browser = HelpBrowser::new(topics.clone().into(), Vec::new(), "");
        assert_eq!(browser.filtered.len(), MAX_FILTERED_RESULTS);
        assert_eq!(browser.filtered[0].0, 0);

        let matches = fuzzy_search_topics(&topics, "same");
        assert_eq!(matches.len(), MAX_FILTERED_RESULTS);
        assert_eq!(matches[0].0.topic, "same0");
    }

    fn legacy_fuzzy_search_topics(topics: &[HelpTopic], query: &str) -> Vec<(HelpTopic, u32)> {
        let mut results: Vec<(HelpTopic, bool, u32)> = topics
            .iter()
            .filter_map(|topic| {
                let name = topic.qualified_name();
                let mut best_rank = None;
                let mut consider = |candidate: &str, weight: u32| {
                    if let Some(matched) = fuzzy_match_with_case_preference(query, candidate) {
                        let rank = (matched.case_preferred, matched.fuzzy_match.score / weight);
                        best_rank =
                            Some(best_rank.map_or(rank, |best: (bool, u32)| best.max(rank)));
                    }
                };
                consider(&name, 1);
                consider(&topic.topic, 1);
                let max_candidate_len = topic
                    .aliases
                    .iter()
                    .map(String::len)
                    .chain(topic.help_key.iter().map(String::len))
                    .max()
                    .unwrap_or(0);
                let mut qualified_candidate =
                    String::with_capacity(topic.package.len() + 2 + max_candidate_len);
                for alias in topic
                    .aliases
                    .iter()
                    .map(String::as_str)
                    .chain(topic.help_key.as_deref())
                {
                    consider(alias, 1);
                    qualified_candidate.clear();
                    qualified_candidate.push_str(&topic.package);
                    qualified_candidate.push_str("::");
                    qualified_candidate.push_str(alias);
                    consider(&qualified_candidate, 1);
                }
                consider(&topic.title, 2);
                best_rank.map(|(case_preferred, score)| (topic.clone(), case_preferred, score))
            })
            .collect();
        results.sort_by_key(|entry| std::cmp::Reverse((entry.1, entry.2)));
        results.truncate(MAX_FILTERED_RESULTS);
        results
            .into_iter()
            .map(|(topic, _, score)| (topic, score))
            .collect()
    }

    #[test]
    #[ignore = "manual optimized-profile search benchmark"]
    fn measure_help_search_before_after_release() {
        use std::hint::black_box;
        use std::time::Instant;

        const TOPICS: usize = 3_000;
        const ALIASES_PER_TOPIC: usize = 3;
        // This prefix series models repeated Backspace edits from "topic" to "t".
        const QUERIES: [&str; 10] = [
            "topic",
            "topi",
            "top",
            "to",
            "t",
            "a",
            "ToPi",
            "Topic",
            "package_1::topic_1",
            "xyz",
        ];
        const REPEATS: usize = 5;
        let fixture_start = Instant::now();
        let topics: Vec<_> = (0..TOPICS)
            .map(|index| {
                let aliases = (0..ALIASES_PER_TOPIC)
                    .map(|alias| format!("topic_{index}_alias_{alias}"))
                    .collect();
                HelpTopic {
                    package: format!("package_{}", index % 60),
                    package_dir: std::path::PathBuf::new(),
                    topic: format!("topic_{index}"),
                    aliases,
                    help_key: Some(format!("help_key_{index}")),
                    title: format!("Topic {index} documentation and examples"),
                    entry_type: "help".to_string(),
                }
            })
            .collect();
        let fixture_preparation = fixture_start.elapsed();

        for query in QUERIES {
            assert_eq!(
                legacy_fuzzy_search_topics(&topics, query),
                fuzzy_search_topics(&topics, query),
                "benchmark implementations differ for {query:?}"
            );
        }

        let mut legacy_timings = Vec::with_capacity(QUERIES.len());
        for query in QUERIES {
            let start = Instant::now();
            for _ in 0..REPEATS {
                black_box(legacy_fuzzy_search_topics(
                    black_box(&topics),
                    black_box(query),
                ));
            }
            legacy_timings.push(start.elapsed());
        }

        let mut optimized_timings = Vec::with_capacity(QUERIES.len());
        for query in QUERIES {
            let start = Instant::now();
            for _ in 0..REPEATS {
                black_box(fuzzy_search_topics(black_box(&topics), black_box(query)));
            }
            optimized_timings.push(start.elapsed());
        }

        eprintln!(
            "fixture preparation: {fixture_preparation:?}; fixture: {TOPICS} topics, {ALIASES_PER_TOPIC} aliases/topic, {} queries, {REPEATS} repeats/query",
            QUERIES.len(),
        );
        for ((query, legacy), optimized) in QUERIES
            .into_iter()
            .zip(legacy_timings)
            .zip(optimized_timings)
        {
            eprintln!("query {query:?}: legacy={legacy:?}, optimized={optimized:?}");
        }
    }

    #[test]
    #[ignore = "manual optimized-profile help browser phase benchmark; requires an installed R executable"]
    fn measure_help_browser_pipeline_phases() {
        use std::hint::black_box;
        use std::time::Instant;

        const FIXTURE_QUERIES: [&str; 10] = [
            "topic",
            "topi",
            "top",
            "to",
            "t",
            "a",
            "ToPi",
            "Topic",
            "package_1::topic_1",
            "xyz",
        ];
        const INSTALLED_QUERIES: [&str; 8] =
            ["print", "prin", "pri", "pr", "p", "mean", "Mean", "xyz123"];
        const FIXTURE_REPEATS: usize = 5;

        let fixture_start = Instant::now();
        let fixture: Vec<_> = (0..3_000)
            .map(|index| {
                let aliases = (0..3)
                    .map(|alias| format!("topic_{index}_alias_{alias}"))
                    .collect();
                HelpTopic {
                    package: format!("package_{}", index % 60),
                    package_dir: std::path::PathBuf::new(),
                    topic: format!("topic_{index}"),
                    aliases,
                    help_key: Some(format!("help_key_{index}")),
                    title: format!("Topic {index} documentation and examples"),
                    entry_type: "help".to_string(),
                }
            })
            .collect();
        let fixture_preparation = fixture_start.elapsed();

        let fixture_index_start = Instant::now();
        let mut fixture_search = SearchBench::new(&fixture);
        let fixture_index_preparation = fixture_index_start.elapsed();
        for query in FIXTURE_QUERIES {
            let legacy = legacy_fuzzy_search_topics(&fixture, query);
            let indexed = fixture_search
                .search(query)
                .into_iter()
                .map(|(index, score)| (fixture[index].clone(), score))
                .collect::<Vec<_>>();
            assert_eq!(legacy, indexed, "search mismatch for {query:?}");
        }
        let fixture_search_times = FIXTURE_QUERIES.map(|query| {
            let start = Instant::now();
            for _ in 0..FIXTURE_REPEATS {
                black_box(fixture_search.search(black_box(query)));
            }
            start.elapsed()
        });

        // Read library paths from a child process; metadata loading and matching stay outside R.
        let library_start = Instant::now();
        let output = std::process::Command::new("R")
            .args(["--vanilla", "--slave", "-e", "writeLines(.libPaths())"])
            .output()
            .expect("run R to read .libPaths()");
        assert!(
            output.status.success(),
            "R .libPaths() failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let libraries = String::from_utf8(output.stdout)
            .expect("R .libPaths() output is UTF-8")
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let library_path_time = library_start.elapsed();
        let metadata_start = Instant::now();
        let installed_topics = get_help_topics_from_paths(&libraries);
        let metadata_time = metadata_start.elapsed();
        let installed_index_start = Instant::now();
        let mut installed_search = SearchBench::new(&installed_topics);
        let installed_index_preparation = installed_index_start.elapsed();
        let installed_alias_count = installed_topics
            .iter()
            .map(|topic| topic.aliases.len())
            .sum::<usize>();
        let installed_search_results = INSTALLED_QUERIES.map(|query| {
            let legacy = legacy_fuzzy_search_topics(&installed_topics, query);
            let indexed = installed_search
                .search(query)
                .into_iter()
                .map(|(index, score)| (installed_topics[index].clone(), score))
                .collect::<Vec<_>>();
            assert_eq!(legacy, indexed, "installed search mismatch for {query:?}");
            let start = Instant::now();
            let matches = black_box(installed_search.search(black_box(query)));
            (start.elapsed(), matches.len())
        });

        let worker_start =
            SearchWorker::spawn(installed_topics.clone().into()).expect("spawn help search worker");
        let mut worker = worker_start;
        let worker_wait_times = INSTALLED_QUERIES.map(|query| {
            let start = Instant::now();
            let generation = worker.submit(query.to_string()).unwrap();
            black_box(worker.wait_for_result(generation).unwrap());
            start.elapsed()
        });
        worker.shutdown_and_join().unwrap();

        let render_results = installed_search.search("print");
        drop(installed_search);
        let installed_topics: Arc<[HelpTopic]> = installed_topics.into();
        let mut browser = HelpBrowser::new(Arc::clone(&installed_topics), libraries, "print");
        browser.filtered = render_results;
        let mut render_buffer = Vec::new();
        let render_start = Instant::now();
        for _ in 0..FIXTURE_REPEATS {
            render_buffer.clear();
            browser.render_to(&mut render_buffer, 100, 30).unwrap();
            black_box(render_buffer.len());
        }
        let render_time = render_start.elapsed();

        eprintln!(
            "fixture: prepare={fixture_preparation:?}, borrowed names+matcher={fixture_index_preparation:?}, {FIXTURE_REPEATS} searches/query"
        );
        for (query, elapsed) in FIXTURE_QUERIES.into_iter().zip(fixture_search_times) {
            eprintln!("fixture search {query:?}: {elapsed:?}");
        }
        eprintln!(
            "installed metadata: child R .libPaths()={library_path_time:?}, get_help_topics={metadata_time:?}; topics={}, aliases={installed_alias_count}",
            installed_topics.len(),
        );
        eprintln!("installed borrowed names+matcher: {installed_index_preparation:?}");
        for ((query, (search, returned)), worker_wait) in INSTALLED_QUERIES
            .into_iter()
            .zip(installed_search_results)
            .zip(worker_wait_times)
        {
            eprintln!(
                "installed search {query:?}: direct={search:?}, returned={returned} (capped at 500), worker submit-to-result={worker_wait:?}"
            );
        }
        eprintln!(
            "render_to buffer: {render_time:?} for {FIXTURE_REPEATS} frames at 100x30; terminal repaint latency is excluded"
        );
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
        let (result, max_scroll) = scroll_display("Hello", 10, 0);
        assert_eq!(result, "Hello");
        assert_eq!(max_scroll, 0);
    }

    #[test]
    fn test_scroll_display_at_start() {
        // "Hello World" (11 cols) with max_width = 8
        let (result, max_scroll) = scroll_display("Hello World", 8, 0);
        assert_eq!(result, "Hello W…");
        // max_scroll = 11 - 7 = 4
        assert_eq!(max_scroll, 4);
    }

    #[test]
    fn test_scroll_display_at_end() {
        let (result, _) = scroll_display("Hello World", 8, 10);
        assert_eq!(result, "…o World");
    }

    #[test]
    fn test_scroll_display_in_middle() {
        let (result, _) = scroll_display("Hello World", 8, 2);
        assert_eq!(result, "…llo Wo…");
    }

    #[test]
    fn test_scroll_display_unicode() {
        // "日本語テスト" = 12 display cols
        // max_width = 7, scroll_pos = 0: first 6 cols + "…"
        let (result, max_scroll) = scroll_display("日本語テスト", 7, 0);
        assert_eq!(result, "日本語…");
        // max_scroll = 12 - 6 = 6
        assert_eq!(max_scroll, 6);

        // At the end: show last 6 cols = "テスト"
        let (result, _) = scroll_display("日本語テスト", 7, 100);
        assert_eq!(result, "…テスト");
    }
}
