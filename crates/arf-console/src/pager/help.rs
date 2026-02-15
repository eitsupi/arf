//! Interactive fuzzy help search for R documentation.
//!
//! This module provides a terminal-based fuzzy search interface for R help topics.
//!
//! # Acknowledgment
//!
//! This implementation is inspired by the **felp** package by Atsushi Yasumoto (atusy):
//! - Repository: <https://github.com/atusy/felp>
//! - CRAN: <https://cran.r-project.org/package=felp>
//!
//! The concept of fuzzy help search and the use of `utils::hsearch_db()` for
//! retrieving the help database were learned from felp's `fuzzyhelp()` function.

use super::text_utils::{
    display_width, exceeds_width, pad_to_width, scroll_display, truncate_to_width,
};
use super::{
    MinimumSize, TextScrollState, check_terminal_too_small, render_size_warning,
    with_alternate_screen,
};
use crate::fuzzy::fuzzy_match;
use arf_harp::help::{HelpTopic, get_help_markdown, get_help_topics, get_vignette_text};
use crossterm::{
    ExecutableCommand, cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind},
    queue,
    style::Stylize,
    terminal::{self, BeginSynchronizedUpdate, EndSynchronizedUpdate},
};
use std::io::{self, Write};
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
                            (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                                if self.selected > 0 {
                                    self.selected -= 1;
                                    if self.selected < self.scroll_offset {
                                        self.scroll_offset = self.selected;
                                    }
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
                                                        display_help_pager(&title, &text)
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
                                                    if let Err(e) = display_help_pager(&title, &msg)
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
                                            if let Err(e) = display_help_pager(&title, &msg) {
                                                log::error!("help_browser: pager error: {}", e);
                                            }
                                        }
                                        _ => {
                                            // "help" and any other types
                                            match get_help_markdown(
                                                &topic.topic,
                                                Some(&topic.package),
                                            ) {
                                                Ok(text) => {
                                                    if let Err(e) =
                                                        display_help_pager(&title, &text)
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

                                    // Force a full redraw after returning from pager
                                    needs_redraw = true;
                                }
                            }

                            // Backspace - delete character before cursor
                            (KeyCode::Backspace, _) => {
                                if self.cursor_pos > 0 {
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
                            }

                            // Delete - delete character at cursor
                            (KeyCode::Delete, _) => {
                                if self.cursor_pos < self.query.chars().count() {
                                    let byte_pos = self
                                        .query
                                        .char_indices()
                                        .nth(self.cursor_pos)
                                        .map(|(i, _)| i)
                                        .unwrap_or(self.query.len());
                                    self.query.remove(byte_pos);
                                    self.update_filter();
                                }
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
                            (KeyCode::Left, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                                if self.cursor_pos > 0 {
                                    self.cursor_pos -= 1;
                                }
                            }
                            (KeyCode::Right, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                                if self.cursor_pos < self.query.chars().count() {
                                    self.cursor_pos += 1;
                                }
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
    let mut results: Vec<(HelpTopic, u32)> = topics
        .iter()
        .filter_map(|topic| {
            // Search in qualified name (package::topic) and title
            let name = topic.qualified_name();
            let name_score = fuzzy_match(query, &name).map(|m| m.score);
            let topic_score = fuzzy_match(query, &topic.topic).map(|m| m.score);
            let title_score = fuzzy_match(query, &topic.title).map(|m| m.score / 2); // Title matches weighted less

            // Take the best score
            let best_score = name_score
                .into_iter()
                .chain(topic_score)
                .chain(title_score)
                .max();

            best_score.map(|score| (topic.clone(), score))
        })
        .collect();

    // Sort by score (descending)
    results.sort_by(|a, b| b.1.cmp(&a.1));

    // Limit results
    results.truncate(MAX_FILTERED_RESULTS);

    results
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
fn display_help_pager(title: &str, content: &str) -> io::Result<()> {
    use super::markdown::render_markdown;
    use super::{PagerAction, PagerConfig, PagerContent, run};
    use ratatui::text::Line;

    struct HelpContent {
        lines: Vec<Line<'static>>,
    }

    impl PagerContent for HelpContent {
        fn line_count(&self) -> usize {
            self.lines.len()
        }

        fn render_line(&self, index: usize, _width: usize) -> Line<'static> {
            self.lines.get(index).cloned().unwrap_or_default()
        }

        fn handle_key(&mut self, code: KeyCode, _modifiers: KeyModifiers) -> Option<PagerAction> {
            if code == KeyCode::Enter {
                Some(PagerAction::Exit)
            } else {
                None
            }
        }
    }

    let mut content = HelpContent {
        lines: render_markdown(content),
    };

    let config = PagerConfig {
        title,
        footer_hint: "↑↓/jk scroll  q/Enter/Esc back",
        manage_alternate_screen: false,
    };

    run(&mut content, &config)
}

#[cfg(test)]
mod tests {
    use super::*;

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
                topic: "print".to_string(),
                title: "Print Values".to_string(),
                entry_type: "help".to_string(),
            },
            HelpTopic {
                package: "dplyr".to_string(),
                topic: "mutate".to_string(),
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
    fn test_fuzzy_search_topics_empty_query() {
        let topics = vec![HelpTopic {
            package: "base".to_string(),
            topic: "print".to_string(),
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
            topic: "print".to_string(),
            title: "Print Values".to_string(),
            entry_type: "help".to_string(),
        }];

        let results = fuzzy_search_topics(&topics, "xyz123");
        assert!(results.is_empty());
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
