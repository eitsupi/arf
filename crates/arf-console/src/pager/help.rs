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
use crate::fuzzy::fuzzy_match_with_case_preference;
use arf_harp::help::{
    HelpTopic, get_help_topics, get_package_help_markdown, get_package_help_markdown_by_key_in_dir,
    get_vignette_text,
};
use arf_harp::help_bridge::PreparedHelpRequest;
use crossterm::{
    ExecutableCommand, cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind},
    queue,
    style::Stylize,
    terminal::{self, BeginSynchronizedUpdate, EndSynchronizedUpdate},
};
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

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
    // Get help topics from R
    let topics = match get_help_topics() {
        Ok(t) => t,
        Err(e) => {
            println!("# Error loading help database: {}", e);
            return Ok(());
        }
    };

    if topics.is_empty() {
        println!("# No help topics found. Make sure R packages are installed.");
        return Ok(());
    }

    let mut browser = HelpBrowser::new(topics, query);
    browser.run()
}

/// Interactive help browser.
struct HelpBrowser {
    topics: Vec<HelpTopic>,
    query: String,
    /// Cursor position within the query string (in characters, not bytes).
    cursor_pos: usize,
    filtered: Vec<(HelpTopic, u32)>,
    selected: usize,
    scroll_offset: usize,
    /// Scroll animation state for the selected item's long text.
    text_scroll: TextScrollState,
}

impl HelpBrowser {
    fn new(topics: Vec<HelpTopic>, query: &str) -> Self {
        let mut browser = HelpBrowser {
            topics,
            query: query.to_string(),
            cursor_pos: query.chars().count(),
            filtered: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            text_scroll: TextScrollState::new(),
        };
        browser.update_filter();
        browser
    }

    fn update_filter(&mut self) {
        if self.query.is_empty() {
            // Show all topics sorted by package then topic
            self.filtered = self.topics.iter().map(|t| (t.clone(), 0)).collect();
            // Limit to avoid memory issues
            self.filtered.truncate(MAX_FILTERED_RESULTS);
        } else {
            self.filtered = fuzzy_search_topics(&self.topics, &self.query);
        }
        self.selected = 0;
        self.scroll_offset = 0;
        // Ensure cursor_pos stays within bounds
        let query_len = self.query.chars().count();
        if self.cursor_pos > query_len {
            self.cursor_pos = query_len;
        }
    }

    fn run(&mut self) -> io::Result<()> {
        with_alternate_screen(|| self.run_inner())
    }

    fn run_inner(&mut self) -> io::Result<()> {
        let mut stdout = io::stdout();
        let poll_timeout = Duration::from_millis(50); // ~20fps for smooth animation
        let mut needs_redraw = true;
        let mut too_small;

        loop {
            // Update animation state
            if self.update_text_scroll() {
                needs_redraw = true;
            }

            too_small = check_terminal_too_small(&MIN_SIZE).is_some();
            if needs_redraw {
                self.render(&mut stdout)?;
                needs_redraw = false;
            }

            // Poll for events with timeout to allow animation updates
            if event::poll(poll_timeout)? {
                let ev = event::read()?;
                log::debug!("help_browser: received event: {:?}", ev);
                match ev {
                    Event::Key(key) => {
                        // Only handle key press events, ignore release and repeat
                        // This is important on Windows where release events are sent
                        // (e.g., Enter release from the command that launched the browser)
                        if key.kind != KeyEventKind::Press {
                            log::debug!(
                                "help_browser: ignoring non-press key event: {:?}",
                                key.kind
                            );
                            continue;
                        }
                        needs_redraw = true;
                        log::debug!(
                            "help_browser: key event: code={:?}, modifiers={:?}",
                            key.code,
                            key.modifiers
                        );

                        // When the terminal is too small, only accept exit keys
                        // to prevent character input from leaking into the filter.
                        if too_small {
                            match (key.code, key.modifiers) {
                                (KeyCode::Esc, _)
                                | (KeyCode::Char('q'), KeyModifiers::NONE)
                                | (KeyCode::Char('c'), KeyModifiers::CONTROL)
                                | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                    break;
                                }
                                _ => continue,
                            }
                        }

                        match (key.code, key.modifiers) {
                            // Exit
                            (KeyCode::Esc, _)
                            | (KeyCode::Char('c'), KeyModifiers::CONTROL)
                            | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                break;
                            }

                            // Navigation
                            (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL)
                                if self.selected > 0 =>
                            {
                                self.selected -= 1;
                                if self.selected < self.scroll_offset {
                                    self.scroll_offset = self.selected;
                                }
                            }
                            (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                                let visible_rows = visible_result_rows();
                                if self.selected + 1 < self.filtered.len() {
                                    self.selected += 1;
                                    if self.selected >= self.scroll_offset + visible_rows {
                                        self.scroll_offset = self.selected - visible_rows + 1;
                                    }
                                }
                            }

                            // Select
                            (KeyCode::Enter, _) | (KeyCode::Tab, _) => {
                                if let Some((topic, _)) = self.filtered.get(self.selected) {
                                    let title = topic.qualified_name();

                                    match topic.entry_type.as_str() {
                                        "vignette" => {
                                            match get_vignette_text(&topic.topic, &topic.package) {
                                                Ok(text) => {
                                                    if let Err(e) =
                                                        display_help_pager(&title, &text, false)
                                                    {
                                                        log::error!(
                                                            "help_browser: pager error: {}",
                                                            e
                                                        );
                                                    }
                                                }
                                                Err(e) => {
                                                    // Show the error (e.g. PDF vignette message)
                                                    // in the pager for visibility
                                                    let msg = format!("{}", e);
                                                    if let Err(e) =
                                                        display_help_pager(&title, &msg, false)
                                                    {
                                                        log::error!(
                                                            "help_browser: pager error: {}",
                                                            e
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        "demo" => {
                                            let msg = format!(
                                                r#"This is a demo entry.

To run the demo, execute in R:

demo("{name}", package = "{pkg}")"#,
                                                name = topic.topic,
                                                pkg = topic.package,
                                            );
                                            if let Err(e) = display_help_pager(&title, &msg, false)
                                            {
                                                log::error!("help_browser: pager error: {}", e);
                                            }
                                        }
                                        _ => {
                                            // "help" and any other types
                                            if let Some(key) = topic.help_key.as_deref() {
                                                if let Err(e) = display_help_page_by_key_in_browser(
                                                    &topic.package_dir,
                                                    &topic.topic,
                                                    key,
                                                    &topic.package,
                                                ) {
                                                    let message = help_page_load_error_message(&e);
                                                    if let Err(pager_error) =
                                                        display_help_pager(&title, &message, false)
                                                    {
                                                        log::error!(
                                                            "help_browser: failed to display help error: {}",
                                                            pager_error
                                                        );
                                                    }
                                                }
                                            } else {
                                                match get_package_help_markdown(
                                                    &topic.topic,
                                                    &topic.package,
                                                ) {
                                                    Ok(text) => {
                                                        if let Err(e) =
                                                            display_help_pager(&title, &text, false)
                                                        {
                                                            log::error!(
                                                                "help_browser: pager error: {}",
                                                                e
                                                            );
                                                        }
                                                    }
                                                    Err(e) => {
                                                        log::error!(
                                                            "help_browser: failed to get help: {}",
                                                            e
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    // Force a full redraw after returning from pager
                                    needs_redraw = true;
                                }
                            }

                            // Backspace - delete character before cursor
                            (KeyCode::Backspace, _) if self.cursor_pos > 0 => {
                                // Find byte position of character before cursor
                                let byte_pos = self
                                    .query
                                    .char_indices()
                                    .nth(self.cursor_pos - 1)
                                    .map(|(i, _)| i)
                                    .unwrap_or(0);
                                self.query.remove(byte_pos);
                                self.cursor_pos -= 1;
                                self.update_filter();
                            }

                            // Delete - delete character at cursor
                            (KeyCode::Delete, _)
                                if self.cursor_pos < self.query.chars().count() =>
                            {
                                let byte_pos = self
                                    .query
                                    .char_indices()
                                    .nth(self.cursor_pos)
                                    .map(|(i, _)| i)
                                    .unwrap_or(self.query.len());
                                self.query.remove(byte_pos);
                                self.update_filter();
                            }

                            // Clear query
                            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                                self.query.clear();
                                self.cursor_pos = 0;
                                self.update_filter();
                            }

                            // Character input
                            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                                // Insert at cursor position
                                let byte_pos = self
                                    .query
                                    .char_indices()
                                    .nth(self.cursor_pos)
                                    .map(|(i, _)| i)
                                    .unwrap_or(self.query.len());
                                self.query.insert(byte_pos, c);
                                self.cursor_pos += 1;
                                self.update_filter();
                            }

                            // Cursor movement
                            (KeyCode::Left, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL)
                                if self.cursor_pos > 0 =>
                            {
                                self.cursor_pos -= 1;
                            }
                            (KeyCode::Right, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL)
                                if self.cursor_pos < self.query.chars().count() =>
                            {
                                self.cursor_pos += 1;
                            }
                            (KeyCode::Home, _) | (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                                self.cursor_pos = 0;
                            }
                            (KeyCode::End, _) | (KeyCode::Char('e'), KeyModifiers::CONTROL) => {
                                self.cursor_pos = self.query.chars().count();
                            }

                            _ => {}
                        }
                    }
                    // Handle mouse scroll events
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            needs_redraw = true;
                            if self.selected > 0 {
                                self.selected -= 1;
                                if self.selected < self.scroll_offset {
                                    self.scroll_offset = self.selected;
                                }
                            }
                        }
                        MouseEventKind::ScrollDown => {
                            needs_redraw = true;
                            let visible_rows = visible_result_rows();
                            if self.selected + 1 < self.filtered.len() {
                                self.selected += 1;
                                if self.selected >= self.scroll_offset + visible_rows {
                                    self.scroll_offset = self.selected - visible_rows + 1;
                                }
                            }
                        }
                        // Ignore other mouse events (move, drag, click) - no redraw needed
                        _ => {}
                    },
                    // Handle resize events
                    Event::Resize(_, _) => {
                        needs_redraw = true;
                    }
                    // Ignore other events (focus, paste)
                    _ => {}
                }
            }
        }

        Ok(())
    }

    /// Update the text scroll animation state.
    fn update_text_scroll(&mut self) -> bool {
        self.text_scroll.update(self.selected)
    }

    fn render(&self, stdout: &mut io::Stdout) -> io::Result<()> {
        if let Some((cols, rows)) = check_terminal_too_small(&MIN_SIZE) {
            return render_size_warning(stdout, cols, rows, &MIN_SIZE);
        }

        // Begin synchronized update to prevent flickering
        queue!(stdout, BeginSynchronizedUpdate)?;

        // Move cursor to top-left and hide it
        stdout.execute(cursor::MoveTo(0, 0))?;
        stdout.execute(cursor::Hide)?;

        // Get terminal size
        let (cols, _rows) = terminal::size().unwrap_or((80, 24));
        let width = cols as usize;

        // Header
        let header = format!("─ Help Search [{} topics] ─", self.filtered.len());
        let padded_header = format!("{:─<width$}", header, width = width);
        println!("\r{}", padded_header.dark_grey());

        // Query input with cursor at correct position
        let before_cursor: String = self.query.chars().take(self.cursor_pos).collect();
        let after_cursor: String = self.query.chars().skip(self.cursor_pos).collect();
        let query_line = format!("  Filter: {}_{}", before_cursor, after_cursor);
        println!("\r{}", pad_to_width(&query_line, width));

        // Separator
        println!("\r{}", "─".repeat(width).dark_grey());

        // Results
        let (name_width, title_width) = calculate_layout(width);
        let visible_rows = visible_result_rows();

        for i in 0..visible_rows {
            let idx = self.scroll_offset + i;
            if idx < self.filtered.len() {
                let (topic, _score) = &self.filtered[idx];
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
                    println!("\r{}", line.reverse());
                } else {
                    // Apply dark_grey only to the title portion for non-selected items
                    let name_part = format!("{}{} ", prefix, padded_name);
                    let title_part = truncate_to_width(
                        &display_title,
                        width.saturating_sub(display_width(&name_part)),
                    );
                    let padding_len = width
                        .saturating_sub(display_width(&name_part) + display_width(&title_part));
                    print!(
                        "\r{}{}{}\n",
                        name_part,
                        title_part.dark_grey(),
                        " ".repeat(padding_len)
                    );
                }
            } else {
                println!("\r{}", " ".repeat(width));
            }
        }

        // Footer
        println!("\r{}", "─".repeat(width).dark_grey());

        // Build plain text first, pad it, then apply style.
        // pad_to_width is not ANSI-aware, so styling must come after padding.
        let footer_plain = "  ↑↓ navigate Tab/Enter select Esc exit";
        println!("\r{}", pad_to_width(footer_plain, width).dark_grey());

        // End synchronized update
        queue!(stdout, EndSynchronizedUpdate)?;
        stdout.flush()?;
        Ok(())
    }
}

/// Perform fuzzy search on help topics.
fn fuzzy_search_topics(topics: &[HelpTopic], query: &str) -> Vec<(HelpTopic, u32)> {
    let mut results: Vec<(HelpTopic, bool, u32)> = topics
        .iter()
        .filter_map(|topic| {
            // Search in qualified name (package::topic) and title
            let name = topic.qualified_name();
            let mut best_rank = None;
            let mut consider_candidate = |candidate: &str, weight: u32| {
                if let Some(matched) = fuzzy_match_with_case_preference(query, candidate) {
                    let score = matched.fuzzy_match.score / weight;
                    let rank = (matched.case_preferred, score);
                    best_rank = Some(best_rank.map_or(rank, |best: (bool, u32)| best.max(rank)));
                }
            };

            consider_candidate(&name, 1);
            consider_candidate(&topic.topic, 1);
            let max_candidate_len = topic
                .aliases
                .iter()
                .map(String::len)
                .chain(topic.help_key.iter().map(String::len))
                .max()
                .unwrap_or(0);
            let mut qualified_candidate =
                String::with_capacity(topic.package.len() + 2 + max_candidate_len);
            for candidate in topic
                .aliases
                .iter()
                .map(String::as_str)
                .chain(topic.help_key.as_deref())
            {
                consider_candidate(candidate, 1);

                qualified_candidate.clear();
                qualified_candidate.push_str(&topic.package);
                qualified_candidate.push_str("::");
                qualified_candidate.push_str(candidate);
                consider_candidate(&qualified_candidate, 1);
            }
            consider_candidate(&topic.title, 2); // Title matches weighted less

            best_rank.map(|(case_preferred, score)| (topic.clone(), case_preferred, score))
        })
        .collect();

    // Prefer smart-case matches, then sort by the existing fuzzy score.
    results.sort_by_key(|entry| std::cmp::Reverse((entry.1, entry.2)));

    // Limit results
    results.truncate(MAX_FILTERED_RESULTS);

    results
        .into_iter()
        .map(|(topic, _, score)| (topic, score))
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
    // Reserve 5 lines for UI chrome (header, filter, 2 separators, footer)
    (rows as usize).saturating_sub(5).max(3)
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
    let selected = match request.pages.as_slice() {
        [] => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "prepared help request has no pages",
            ));
        }
        [_] => 0,
        pages => {
            let Some(selected) = select_prepared_help_page(pages)? else {
                return Ok(());
            };
            selected
        }
    };

    let page = &request.pages[selected];
    display_help_pager(
        &help_page_title(&page.package, &page.display_topic),
        &page.markdown,
        true,
    )
}

fn select_prepared_help_page(
    pages: &[arf_harp::help_bridge::PreparedHelpPage],
) -> io::Result<Option<usize>> {
    use super::{PagerAction, PagerConfig, PagerContent, run};
    use ratatui::text::Line;

    struct PageSelector {
        labels: Vec<String>,
        state: HelpPageSelectorState,
    }

    impl PagerContent for PageSelector {
        fn line_count(&self) -> usize {
            self.labels.len()
        }

        fn render_line(&self, index: usize, _width: usize) -> Line<'static> {
            let prefix = if self.state.selected == Some(index) {
                ">"
            } else {
                " "
            };
            Line::from(format!("{prefix} {}", self.labels[index]))
        }

        fn handle_key(&mut self, code: KeyCode, _modifiers: KeyModifiers) -> Option<PagerAction> {
            match code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.state.move_up().then_some(PagerAction::Continue)
                }
                KeyCode::Down | KeyCode::Char('j') => match self.state.move_down() {
                    Some(true) => Some(PagerAction::Redraw),
                    Some(false) => Some(PagerAction::Continue),
                    None => Some(PagerAction::Redraw),
                },
                KeyCode::Enter => self
                    .state
                    .confirm()
                    .map_or(Some(PagerAction::Redraw), |_| Some(PagerAction::Exit)),
                _ => None,
            }
        }
    }

    if pages.len() < 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "page selector requires multiple candidates",
        ));
    }

    let labels = pages
        .iter()
        .enumerate()
        .map(|(index, page)| format!("[{}] {}::{}", index + 1, page.package, page.display_topic))
        .collect();
    let mut selector = PageSelector {
        labels,
        state: HelpPageSelectorState::new(pages.len()),
    };
    let config = PagerConfig {
        title: "Select R help page",
        footer_hint: "↑↓/jk move  Enter open  q/Esc cancel",
        manage_alternate_screen: true,
    };
    run(&mut selector, &config)?;
    Ok(selector.state.confirmed)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HelpPageSelectorState {
    page_count: usize,
    selected: Option<usize>,
    confirmed: Option<usize>,
}

impl HelpPageSelectorState {
    fn new(page_count: usize) -> Self {
        Self {
            page_count,
            selected: None,
            confirmed: None,
        }
    }

    fn move_up(&mut self) -> bool {
        let Some(selected) = self.selected else {
            return false;
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
    fn move_down(&mut self) -> Option<bool> {
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

    fn confirm(&mut self) -> Option<usize> {
        let selected = self.selected?;
        self.confirmed = Some(selected);
        Some(selected)
    }
}

/// Load a help page by its compiled-help key and display it in the help pager.
///
/// The browser's visible topic can differ from the compiled-help key, so both
/// values and the supplying package directory are passed explicitly. Errors
/// are returned to let callers choose a fallback when the indexed key cannot
/// be resolved.
#[allow(dead_code, reason = "Consumed by the deferred R-help bridge")]
pub(crate) fn display_help_page_by_key(
    package_dir: &Path,
    display_topic: &str,
    help_key: &str,
    package: &str,
) -> io::Result<()> {
    display_help_page_by_key_with_screen(package_dir, display_topic, help_key, package, true)
}

/// Display a compiled-key help page from inside the help browser's alternate screen.
fn display_help_page_by_key_in_browser(
    package_dir: &Path,
    display_topic: &str,
    help_key: &str,
    package: &str,
) -> io::Result<()> {
    display_help_page_by_key_with_screen(package_dir, display_topic, help_key, package, false)
}

fn display_help_page_by_key_with_screen(
    package_dir: &Path,
    display_topic: &str,
    help_key: &str,
    package: &str,
    manage_alternate_screen: bool,
) -> io::Result<()> {
    let content =
        get_package_help_markdown_by_key_in_dir(package_dir, display_topic, help_key, package)
            .map_err(io::Error::other)?;
    display_help_pager(
        &help_page_title(package, display_topic),
        &content,
        manage_alternate_screen,
    )
}

fn help_page_title(package: &str, display_topic: &str) -> String {
    format!("{package}::{display_topic}")
}

fn help_page_load_error_message(error: &io::Error) -> String {
    format!("Unable to load this help topic.\n\n{error}")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut selector = HelpPageSelectorState::new(2);
        assert!(!selector.move_up());
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
