//! Interactive history browser for viewing and managing command history.
//!
//! This module provides a terminal-based browser for viewing, filtering,
//! and batch-deleting command history entries stored in SQLite.

use super::copy_to_clipboard;
use super::text_utils::{
    display_width, exceeds_width, pad_to_width, scroll_display, truncate_to_width,
};
use crate::fuzzy::fuzzy_match;
use chrono::TimeZone;
use crossterm::{
    ExecutableCommand, cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind,
    },
    queue,
    style::Stylize,
    terminal::{
        self, BeginSynchronizedUpdate, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use reedline::{HistoryItem, HistoryItemId};
use rusqlite::{Connection, OpenFlags, params_from_iter};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Maximum number of history entries to load from database.
const MAX_ENTRIES: i64 = 10000;

/// Animation scroll speed in milliseconds per character.
const SCROLL_INTERVAL_MS: u64 = 150;

/// Pause duration at the start and end of scroll animation (in ms).
const SCROLL_PAUSE_MS: u64 = 1000;

/// Database mode for history browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryDbMode {
    /// R command history.
    R,
    /// Shell command history.
    Shell,
}

impl HistoryDbMode {
    /// Display name for the mode.
    pub fn display_name(&self) -> &'static str {
        match self {
            HistoryDbMode::R => "R",
            HistoryDbMode::Shell => "Shell",
        }
    }
}

/// Result of running the history browser.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Variants provide useful API even if not all fields are read
pub enum HistoryBrowserResult {
    /// User exited without action.
    Cancelled,
    /// User copied a command to clipboard.
    Copied(String),
}

/// Parsed filter with optional prefix filters.
#[derive(Debug, Default)]
struct HistoryFilter {
    /// Raw query string (for display).
    raw_query: String,
    /// Cursor position in the query string.
    cursor_pos: usize,
    /// Hostname filter (from `host:xxx`).
    hostname: Option<String>,
    /// CWD prefix filter (from `cwd:/path`).
    cwd_prefix: Option<String>,
    /// Exit status filter (from `exit:N`).
    exit_status: Option<i64>,
    /// Command pattern for fuzzy search (remaining text after prefix filters).
    command_pattern: String,
}

impl HistoryFilter {
    /// Parse a query string into filter components.
    fn parse(query: &str) -> Self {
        let mut filter = HistoryFilter {
            raw_query: query.to_string(),
            cursor_pos: query.chars().count(),
            ..Default::default()
        };

        let mut remaining_parts = Vec::new();

        for part in query.split_whitespace() {
            if let Some(hostname) = part.strip_prefix("host:") {
                filter.hostname = Some(hostname.to_string());
            } else if let Some(cwd) = part.strip_prefix("cwd:") {
                filter.cwd_prefix = Some(cwd.to_string());
            } else if let Some(status) = part.strip_prefix("exit:") {
                if let Ok(n) = status.parse::<i64>() {
                    filter.exit_status = Some(n);
                } else {
                    remaining_parts.push(part);
                }
            } else {
                remaining_parts.push(part);
            }
        }

        filter.command_pattern = remaining_parts.join(" ");
        filter
    }

    /// Re-parse prefix filters from the current raw_query.
    /// Call this after modifying `raw_query` or `cursor_pos` in-place.
    fn reparse(&mut self) {
        let parsed = Self::parse(&self.raw_query);
        self.hostname = parsed.hostname;
        self.cwd_prefix = parsed.cwd_prefix;
        self.exit_status = parsed.exit_status;
        self.command_pattern = parsed.command_pattern;
    }
}

/// A history item with selection state.
struct BrowsableHistoryItem {
    /// The actual history item.
    item: HistoryItem,
    /// Whether this item is selected for deletion.
    selected: bool,
}

/// Interactive history browser.
struct HistoryBrowser {
    /// All loaded history entries.
    entries: Vec<BrowsableHistoryItem>,
    /// Filtered entries as (index, score) pairs.
    filtered: Vec<(usize, u32)>,
    /// Current filter state.
    filter: HistoryFilter,
    /// Cursor position in the list.
    cursor: usize,
    /// Scroll offset for the list.
    scroll_offset: usize,
    /// Feedback message to display.
    feedback_message: Option<String>,
    /// Database mode (R or Shell).
    db_mode: HistoryDbMode,
    /// Path to the history database.
    db_path: PathBuf,
    /// Current horizontal scroll position for text animation.
    text_scroll_pos: usize,
    /// Time when the text scroll animation started.
    text_scroll_start: Instant,
    /// Previously selected cursor position.
    prev_cursor: usize,
    /// Whether we're showing the delete confirmation dialog.
    show_delete_dialog: bool,
    /// Whether filter input mode is active.
    /// When true, all character input goes to the filter text.
    /// When false, single-char keybindings (q, d, y, etc.) work as navigation/commands.
    filter_active: bool,
    /// Cached count of selected entries (maintained by toggle/select/unselect/delete).
    cached_selected_count: usize,
}

impl HistoryBrowser {
    /// Create a new history browser.
    fn new(entries: Vec<HistoryItem>, db_mode: HistoryDbMode, db_path: PathBuf) -> Self {
        let browsable: Vec<BrowsableHistoryItem> = entries
            .into_iter()
            .map(|item| BrowsableHistoryItem {
                item,
                selected: false,
            })
            .collect();
        let filtered: Vec<(usize, u32)> =
            browsable.iter().enumerate().map(|(i, _)| (i, 0)).collect();

        HistoryBrowser {
            entries: browsable,
            filtered,
            filter: HistoryFilter::default(),
            cursor: 0,
            scroll_offset: 0,
            feedback_message: None,
            db_mode,
            db_path,
            text_scroll_pos: 0,
            text_scroll_start: Instant::now(),
            prev_cursor: 0,
            show_delete_dialog: false,
            filter_active: false,
            cached_selected_count: 0,
        }
    }

    /// Update the filtered list based on the current filter.
    fn update_filter(&mut self) {
        if self.filter.command_pattern.is_empty()
            && self.filter.hostname.is_none()
            && self.filter.cwd_prefix.is_none()
            && self.filter.exit_status.is_none()
        {
            // No filter - show all entries
            self.filtered = self
                .entries
                .iter()
                .enumerate()
                .map(|(i, _)| (i, 0))
                .collect();
        } else {
            let mut results: Vec<(usize, u32)> = self
                .entries
                .iter()
                .enumerate()
                .filter_map(|(idx, entry)| {
                    // Apply hostname filter
                    if let Some(ref hostname) = self.filter.hostname {
                        if let Some(ref item_host) = entry.item.hostname {
                            if !item_host.contains(hostname) {
                                return None;
                            }
                        } else {
                            return None;
                        }
                    }

                    // Apply cwd prefix filter
                    if let Some(ref cwd_prefix) = self.filter.cwd_prefix {
                        if let Some(ref item_cwd) = entry.item.cwd {
                            if !item_cwd.starts_with(cwd_prefix) {
                                return None;
                            }
                        } else {
                            return None;
                        }
                    }

                    // Apply exit status filter
                    if let Some(exit_status) = self.filter.exit_status
                        && entry.item.exit_status != Some(exit_status)
                    {
                        return None;
                    }

                    // Apply fuzzy command pattern filter
                    if !self.filter.command_pattern.is_empty() {
                        if let Some(m) =
                            fuzzy_match(&self.filter.command_pattern, &entry.item.command_line)
                        {
                            return Some((idx, m.score));
                        }
                        return None;
                    }

                    Some((idx, 0))
                })
                .collect();

            // Sort by score (descending) if we have fuzzy scores
            if !self.filter.command_pattern.is_empty() {
                results.sort_by(|a, b| b.1.cmp(&a.1));
            }

            self.filtered = results;
        }

        // Reset cursor and scroll
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Count of currently selected items (cached).
    fn selected_count(&self) -> usize {
        self.cached_selected_count
    }

    /// Toggle selection for the item at cursor.
    fn toggle_selection(&mut self) {
        if let Some(&(idx, _)) = self.filtered.get(self.cursor) {
            let entry = &mut self.entries[idx];
            entry.selected = !entry.selected;
            if entry.selected {
                self.cached_selected_count += 1;
            } else {
                self.cached_selected_count = self.cached_selected_count.saturating_sub(1);
            }
        }
    }

    /// Select all visible (filtered) items.
    fn select_all_visible(&mut self) {
        for &(idx, _) in &self.filtered {
            if !self.entries[idx].selected {
                self.entries[idx].selected = true;
                self.cached_selected_count += 1;
            }
        }
    }

    /// Unselect all items.
    fn unselect_all(&mut self) {
        for entry in &mut self.entries {
            entry.selected = false;
        }
        self.cached_selected_count = 0;
    }

    /// Delete all selected items from the database.
    ///
    /// Opens a separate read-write connection and executes a single batch DELETE.
    /// This avoids using `SqliteBackedHistory::with_file()` which would create a
    /// competing WAL connection alongside the main REPL's history connection,
    /// risking cache inconsistency and database corruption.
    fn delete_selected(&mut self) -> io::Result<()> {
        // Collect IDs to delete
        let ids_to_delete: Vec<i64> = self
            .entries
            .iter()
            .filter(|e| e.selected)
            .filter_map(|e| e.item.id)
            .map(|id| id.0)
            .collect();

        if ids_to_delete.is_empty() {
            return Ok(());
        }

        // Open a direct connection for the delete operation only.
        // We intentionally avoid SqliteBackedHistory::with_file() here because it
        // sets journal_mode=wal and runs DDL (CREATE TABLE IF NOT EXISTS), which
        // conflicts with the main REPL's active WAL connection to the same database.
        let db = Connection::open(&self.db_path).map_err(io::Error::other)?;
        db.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(io::Error::other)?;

        // Batch delete in a single statement
        let placeholders: Vec<&str> = ids_to_delete.iter().map(|_| "?").collect();
        let sql = format!(
            "DELETE FROM history WHERE id IN ({})",
            placeholders.join(", ")
        );
        db.execute(&sql, params_from_iter(&ids_to_delete))
            .map_err(io::Error::other)?;

        // Remove deleted entries from our list
        let feedback = format!("Deleted {} entries", ids_to_delete.len());
        self.entries.retain(|e| !e.selected);
        self.cached_selected_count = 0;

        // Rebuild filtered list
        self.update_filter();

        // Adjust cursor if needed
        if self.cursor >= self.filtered.len() && !self.filtered.is_empty() {
            self.cursor = self.filtered.len() - 1;
        }

        self.feedback_message = Some(feedback);
        Ok(())
    }

    /// Move cursor up by one row.
    fn move_cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            if self.cursor < self.scroll_offset {
                self.scroll_offset = self.cursor;
            }
        }
    }

    /// Move cursor down by one row.
    fn move_cursor_down(&mut self) {
        let visible_rows = visible_result_rows();
        if self.cursor + 1 < self.filtered.len() {
            self.cursor += 1;
            if self.cursor >= self.scroll_offset + visible_rows {
                self.scroll_offset = self.cursor - visible_rows + 1;
            }
        }
    }

    /// Move cursor up by one page.
    fn move_page_up(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let page_size = visible_result_rows();
        self.cursor = self.cursor.saturating_sub(page_size);
        self.scroll_offset = self.scroll_offset.saturating_sub(page_size);
    }

    /// Move cursor down by one page.
    fn move_page_down(&mut self) {
        if self.filtered.is_empty() {
            return;
        }
        let page_size = visible_result_rows();
        let max_cursor = self.filtered.len() - 1;
        self.cursor = (self.cursor + page_size).min(max_cursor);
        let max_scroll = self.filtered.len().saturating_sub(page_size);
        self.scroll_offset = (self.scroll_offset + page_size).min(max_scroll);
    }

    /// Move cursor to the first entry.
    fn move_to_top(&mut self) {
        self.cursor = 0;
        self.scroll_offset = 0;
    }

    /// Move cursor to the last entry.
    fn move_to_bottom(&mut self) {
        if !self.filtered.is_empty() {
            self.cursor = self.filtered.len() - 1;
            let visible_rows = visible_result_rows();
            if self.cursor >= visible_rows {
                self.scroll_offset = self.cursor - visible_rows + 1;
            }
        }
    }

    /// Get the command line at the current cursor position.
    fn current_command(&self) -> Option<&str> {
        self.filtered
            .get(self.cursor)
            .map(|&(idx, _)| self.entries[idx].item.command_line.as_str())
    }

    /// Run the browser and return the result.
    fn run(&mut self) -> io::Result<HistoryBrowserResult> {
        let mut stdout = io::stdout();

        stdout.execute(EnterAlternateScreen)?;
        stdout.execute(EnableMouseCapture)?;
        terminal::enable_raw_mode()?;

        let result = self.run_inner();

        terminal::disable_raw_mode()?;
        stdout.execute(DisableMouseCapture)?;
        stdout.execute(cursor::Show)?;
        stdout.execute(LeaveAlternateScreen)?;

        result
    }

    fn run_inner(&mut self) -> io::Result<HistoryBrowserResult> {
        let mut stdout = io::stdout();
        let poll_timeout = Duration::from_millis(50);
        let mut needs_redraw = true;

        loop {
            // Update animation state
            if self.update_text_scroll() {
                needs_redraw = true;
            }

            if needs_redraw {
                self.render(&mut stdout)?;
                needs_redraw = false;
            }

            if event::poll(poll_timeout)? {
                let ev = event::read()?;
                log::debug!("history_browser: received event: {:?}", ev);
                match ev {
                    Event::Key(key) => {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        needs_redraw = true;
                        self.feedback_message = None;

                        // Handle delete confirmation dialog
                        if self.show_delete_dialog {
                            match key.code {
                                KeyCode::Enter => {
                                    self.show_delete_dialog = false;
                                    self.delete_selected()?;
                                }
                                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                                    self.show_delete_dialog = false;
                                    self.feedback_message = Some("Delete cancelled".to_string());
                                }
                                _ => {}
                            }
                            continue;
                        }

                        if self.filter_active {
                            // Filter mode: all char input goes to filter text
                            match (key.code, key.modifiers) {
                                // Confirm filter and return to normal mode
                                (KeyCode::Enter, _) => {
                                    self.filter_active = false;
                                }

                                // Clear filter and return to normal mode
                                (KeyCode::Esc, _) => {
                                    self.filter.raw_query.clear();
                                    self.filter.cursor_pos = 0;
                                    self.filter.reparse();
                                    self.update_filter();
                                    self.filter_active = false;
                                }

                                // Ctrl+C exits the browser entirely
                                (KeyCode::Char('c'), KeyModifiers::CONTROL)
                                | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                    return Ok(HistoryBrowserResult::Cancelled);
                                }

                                // Navigation still works in filter mode
                                (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                                    self.move_cursor_up();
                                }
                                (KeyCode::Down, _)
                                | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                                    self.move_cursor_down();
                                }
                                // PageUp/PageDown only (no Ctrl+B/F which conflict
                                // with Emacs cursor movement in text input context)
                                (KeyCode::PageUp, _) => {
                                    self.move_page_up();
                                }
                                (KeyCode::PageDown, _) => {
                                    self.move_page_down();
                                }
                                // Alt+Home/End: list navigation
                                (KeyCode::Home, m) if m.contains(KeyModifiers::ALT) => {
                                    self.move_to_top();
                                }
                                (KeyCode::End, m) if m.contains(KeyModifiers::ALT) => {
                                    self.move_to_bottom();
                                }
                                // Plain Home/End: move cursor within filter input
                                (KeyCode::Home, _) => {
                                    self.filter.cursor_pos = 0;
                                }
                                (KeyCode::End, _) => {
                                    self.filter.cursor_pos = self.filter.raw_query.chars().count();
                                }
                                (KeyCode::Tab, _) => {
                                    self.toggle_selection();
                                }
                                (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                                    self.select_all_visible();
                                }
                                (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                                    self.unselect_all();
                                }

                                // Backspace
                                (KeyCode::Backspace, _) => {
                                    if self.filter.cursor_pos > 0
                                        && let Some((byte_pos, _)) = self
                                            .filter
                                            .raw_query
                                            .char_indices()
                                            .nth(self.filter.cursor_pos - 1)
                                    {
                                        self.filter.raw_query.remove(byte_pos);
                                        self.filter.cursor_pos -= 1;
                                        self.filter.reparse();
                                        self.update_filter();
                                    }
                                }

                                // Delete
                                (KeyCode::Delete, _) => {
                                    if let Some((byte_pos, _)) = self
                                        .filter
                                        .raw_query
                                        .char_indices()
                                        .nth(self.filter.cursor_pos)
                                    {
                                        self.filter.raw_query.remove(byte_pos);
                                        self.filter.reparse();
                                        self.update_filter();
                                    }
                                }

                                // Cursor movement
                                (KeyCode::Left, KeyModifiers::NONE) => {
                                    if self.filter.cursor_pos > 0 {
                                        self.filter.cursor_pos -= 1;
                                    }
                                }
                                (KeyCode::Right, KeyModifiers::NONE) => {
                                    let query_len = self.filter.raw_query.chars().count();
                                    if self.filter.cursor_pos < query_len {
                                        self.filter.cursor_pos += 1;
                                    }
                                }

                                // Character input
                                (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                                    let byte_pos = self
                                        .filter
                                        .raw_query
                                        .char_indices()
                                        .nth(self.filter.cursor_pos)
                                        .map(|(i, _)| i)
                                        .unwrap_or(self.filter.raw_query.len());
                                    self.filter.raw_query.insert(byte_pos, c);
                                    self.filter.cursor_pos += 1;
                                    self.filter.reparse();
                                    self.update_filter();
                                }

                                _ => {}
                            }
                        } else {
                            // Normal mode: single-char keybindings work
                            match (key.code, key.modifiers) {
                                // Exit
                                (KeyCode::Esc, _) | (KeyCode::Char('q'), KeyModifiers::NONE) => {
                                    return Ok(HistoryBrowserResult::Cancelled);
                                }
                                (KeyCode::Char('c'), KeyModifiers::CONTROL)
                                | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                    return Ok(HistoryBrowserResult::Cancelled);
                                }

                                // Enter filter mode
                                (KeyCode::Char('/'), KeyModifiers::NONE) => {
                                    self.filter_active = true;
                                }

                                // Navigation - up
                                (KeyCode::Up, _)
                                | (KeyCode::Char('k'), KeyModifiers::NONE)
                                | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                                    self.move_cursor_up();
                                }

                                // Navigation - down
                                (KeyCode::Down, _)
                                | (KeyCode::Char('j'), KeyModifiers::NONE)
                                | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                                    self.move_cursor_down();
                                }

                                // Page up
                                (KeyCode::PageUp, _)
                                | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                                    self.move_page_up();
                                }

                                // Page down
                                (KeyCode::PageDown, _)
                                | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                                    self.move_page_down();
                                }

                                // Home / go to top
                                (KeyCode::Home, _) | (KeyCode::Char('g'), KeyModifiers::NONE) => {
                                    self.move_to_top();
                                }

                                // End / go to bottom
                                (KeyCode::End, _) | (KeyCode::Char('G'), KeyModifiers::SHIFT) => {
                                    self.move_to_bottom();
                                }

                                // Toggle selection
                                (KeyCode::Tab, _) => {
                                    self.toggle_selection();
                                }

                                // Toggle selection and move down
                                (KeyCode::Char(' '), KeyModifiers::NONE) => {
                                    self.toggle_selection();
                                    self.move_cursor_down();
                                }

                                // Select all visible
                                (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                                    self.select_all_visible();
                                }

                                // Unselect all
                                (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                                    self.unselect_all();
                                }

                                // Delete selected (show confirmation)
                                (KeyCode::Char('d'), KeyModifiers::NONE) => {
                                    if self.selected_count() > 0 {
                                        self.show_delete_dialog = true;
                                    } else {
                                        self.feedback_message =
                                            Some("No items selected".to_string());
                                    }
                                }

                                // Copy and exit
                                (KeyCode::Enter, _) => {
                                    if let Some(cmd) = self.current_command() {
                                        let cmd = cmd.to_string();
                                        if copy_to_clipboard(&cmd).is_ok() {
                                            return Ok(HistoryBrowserResult::Copied(cmd));
                                        } else {
                                            self.feedback_message =
                                                Some("Failed to copy".to_string());
                                        }
                                    }
                                }

                                // Copy and stay
                                (KeyCode::Char('y'), KeyModifiers::NONE) => {
                                    if let Some(cmd) = self.current_command() {
                                        if copy_to_clipboard(cmd).is_ok() {
                                            self.feedback_message =
                                                Some("Copied to clipboard".to_string());
                                        } else {
                                            self.feedback_message =
                                                Some("Failed to copy".to_string());
                                        }
                                    }
                                }

                                _ => {}
                            }
                        }
                    }
                    Event::Mouse(mouse) => match mouse.kind {
                        MouseEventKind::ScrollUp => {
                            needs_redraw = true;
                            self.move_cursor_up();
                        }
                        MouseEventKind::ScrollDown => {
                            needs_redraw = true;
                            self.move_cursor_down();
                        }
                        _ => {}
                    },
                    Event::Resize(_, _) => {
                        needs_redraw = true;
                    }
                    _ => {}
                }
            }
        }
    }

    /// Update the text scroll animation state.
    fn update_text_scroll(&mut self) -> bool {
        if self.cursor != self.prev_cursor {
            self.prev_cursor = self.cursor;
            self.text_scroll_pos = 0;
            self.text_scroll_start = Instant::now();
            return true;
        }

        let elapsed = self.text_scroll_start.elapsed();

        if elapsed < Duration::from_millis(SCROLL_PAUSE_MS) {
            return false;
        }

        let scroll_time = elapsed - Duration::from_millis(SCROLL_PAUSE_MS);
        let new_pos = (scroll_time.as_millis() / SCROLL_INTERVAL_MS as u128) as usize;

        if new_pos != self.text_scroll_pos {
            self.text_scroll_pos = new_pos;
            true
        } else {
            false
        }
    }

    fn render(&self, stdout: &mut io::Stdout) -> io::Result<()> {
        queue!(stdout, BeginSynchronizedUpdate)?;
        stdout.execute(cursor::MoveTo(0, 0))?;
        stdout.execute(cursor::Hide)?;

        let (cols, _rows) = terminal::size().unwrap_or((80, 24));
        let width = cols as usize;

        // Header with mode and entry count
        let selected_count = self.selected_count();
        let selected_info = if selected_count > 0 {
            format!(" [{} selected]", selected_count)
        } else {
            String::new()
        };
        let header = format!(
            "─ History Browser [{}] [{} entries]{} ─",
            self.db_mode.display_name(),
            self.filtered.len(),
            selected_info
        );
        let padded_header = format!("{:─<width$}", header, width = width);
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        println!("\r{}", padded_header.dark_grey());

        // Filter input
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        if self.filter_active {
            // Show cursor in filter mode
            let before_cursor: String = self
                .filter
                .raw_query
                .chars()
                .take(self.filter.cursor_pos)
                .collect();
            let after_cursor: String = self
                .filter
                .raw_query
                .chars()
                .skip(self.filter.cursor_pos)
                .collect();
            let filter_line = format!("  Filter: {}_{}", before_cursor, after_cursor);
            println!("\r{}", pad_to_width(&filter_line, width));
        } else if self.filter.raw_query.is_empty() {
            // No filter text, show placeholder
            println!(
                "\r{}",
                pad_to_width("  Filter: (press / to filter)", width).dark_grey()
            );
        } else {
            // Show filter text without cursor
            let filter_line = format!("  Filter: {}", self.filter.raw_query);
            println!("\r{}", pad_to_width(&filter_line, width));
        }

        // Separator
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        println!("\r{}", "─".repeat(width).dark_grey());

        // Calculate layout
        let (cmd_width, host_width) = calculate_layout(width);
        let visible_rows = visible_result_rows();

        // Results
        for i in 0..visible_rows {
            stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
            let idx = self.scroll_offset + i;
            if idx < self.filtered.len() {
                let (entry_idx, _score) = self.filtered[idx];
                let entry = &self.entries[entry_idx];
                let is_current = idx == self.cursor;

                // Selection checkbox
                let checkbox = if entry.selected { "[x]" } else { "[ ]" };
                let cursor_marker = if is_current { " > " } else { "   " };

                // Format timestamp
                let timestamp = entry
                    .item
                    .start_timestamp
                    .map(|ts| ts.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| "                ".to_string());

                // Command text with scrolling for selected item
                // Convert multiline commands to single line for display
                let cmd = flatten_multiline(&entry.item.command_line);
                let display_cmd = if is_current && exceeds_width(&cmd, cmd_width) {
                    let (scrolled, _) = scroll_display(&cmd, cmd_width, self.text_scroll_pos);
                    scrolled
                } else {
                    truncate_to_width(&cmd, cmd_width)
                };

                // Hostname (truncated)
                let host = entry.item.hostname.as_deref().unwrap_or("");
                let display_host = truncate_to_width(host, host_width);

                // Build prefix (all ASCII, so byte len == display width)
                let prefix = format!("{}{} {}  ", cursor_marker, checkbox, timestamp);
                let padded_cmd = pad_to_width(&display_cmd, cmd_width);

                if is_current {
                    let content = format!("{}{} {}", prefix, padded_cmd, display_host);
                    let line = pad_to_width(&content, width);
                    println!("\r{}", line.reverse());
                } else if entry.selected {
                    let content = format!("{}{} {}", prefix, padded_cmd, display_host);
                    let line = pad_to_width(&content, width);
                    println!("\r{}", line.yellow());
                } else {
                    // Style hostname as dark grey
                    let base_part = format!("{}{} ", prefix, padded_cmd);
                    let host_str = display_host.to_string();
                    let padding_len =
                        width.saturating_sub(display_width(&base_part) + display_width(&host_str));
                    print!(
                        "\r{}{}{}\n",
                        base_part,
                        host_str.dark_grey(),
                        " ".repeat(padding_len)
                    );
                }
            } else {
                println!("\r{}", " ".repeat(width));
            }
        }

        // Footer separator
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        println!("\r{}", "─".repeat(width).dark_grey());

        // Footer line 1: filter syntax help
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        let syntax_help = "  Filter: host:<name> cwd:<path> exit:<N> <text>  (space = AND)";
        println!("\r{}", pad_to_width(syntax_help, width).dark_grey());

        // Footer line 2: keybindings or feedback message
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        if self.show_delete_dialog {
            let dialog_msg = format!(
                "  Delete {} selected entries? (Enter=confirm, Esc=cancel)",
                selected_count
            );
            println!("\r{}", pad_to_width(&dialog_msg, width).yellow().bold());
        } else if let Some(ref msg) = self.feedback_message {
            println!("\r{}", pad_to_width(&format!("  {}", msg), width));
        } else {
            let footer = if self.filter_active {
                "  Enter confirm | Esc clear | ↑↓/PgUp/PgDn navigate | Tab select"
            } else {
                "  / filter | Space/Tab select | d delete | y copy | Enter copy+exit | q exit"
            };
            println!("\r{}", pad_to_width(footer, width).dark_grey());
        }

        queue!(stdout, EndSynchronizedUpdate)?;
        stdout.flush()?;
        Ok(())
    }
}

/// Load history entries from the database in read-only mode.
///
/// Using read-only mode avoids WAL (Write-Ahead Logging) conflicts with the
/// main REPL's history connection. This prevents "database disk image is malformed"
/// errors when browsing history while the REPL is actively using the database.
fn load_history(db_path: &Path) -> io::Result<Vec<HistoryItem>> {
    // Open in read-only mode to avoid WAL conflicts
    let db = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(io::Error::other)?;

    let mut stmt = db
        .prepare(
            "SELECT id, command_line, start_timestamp, hostname, cwd, duration_ms, exit_status
             FROM history
             ORDER BY id DESC
             LIMIT ?",
        )
        .map_err(io::Error::other)?;

    let items = stmt
        .query_map([MAX_ENTRIES], |row| {
            Ok(HistoryItem {
                id: Some(HistoryItemId::new(row.get(0)?)),
                command_line: row.get(1)?,
                start_timestamp: row
                    .get::<_, Option<i64>>(2)?
                    .and_then(|ms| chrono::Utc.timestamp_millis_opt(ms).single()),
                session_id: None,
                hostname: row.get(3)?,
                cwd: row.get(4)?,
                duration: row
                    .get::<_, Option<i64>>(5)?
                    .and_then(|ms| u64::try_from(ms).ok())
                    .map(std::time::Duration::from_millis),
                exit_status: row.get(6)?,
                more_info: None,
            })
        })
        .map_err(io::Error::other)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io::Error::other)?;

    Ok(items)
}

/// Calculate layout widths.
fn calculate_layout(cols: usize) -> (usize, usize) {
    // Layout: " > [x] 2024-01-15 14:32  command...  hostname"
    // Prefix: 3 + checkbox: 3 + space: 1 + timestamp: 16 + spaces: 2 = 25
    let prefix_width = 25;
    let host_width = 15.min(cols / 6); // ~1/6 of screen for hostname
    let cmd_width = cols.saturating_sub(prefix_width + host_width + 1);
    (cmd_width.max(20), host_width)
}

/// Calculate the number of visible result rows.
fn visible_result_rows() -> usize {
    let (_, rows) = terminal::size().unwrap_or((80, 24));
    // Reserve: header(1) + filter(1) + separator(1) + footer_separator(1) + footer(2) = 6
    rows.saturating_sub(6).max(3) as usize
}

/// Convert a multiline string to a single line for display.
/// Replaces newlines with a visible marker (↵) to indicate line breaks.
fn flatten_multiline(s: &str) -> String {
    if s.contains('\n') {
        s.replace('\n', "↵")
    } else {
        s.to_string()
    }
}

/// Run the history browser.
///
/// # Arguments
/// * `db_path` - Path to the SQLite history database
/// * `mode` - The database mode (R or Shell)
///
/// # Returns
/// The result of the browser interaction.
pub fn run_history_browser(
    db_path: &Path,
    mode: HistoryDbMode,
) -> io::Result<HistoryBrowserResult> {
    // Load history entries
    let entries = load_history(db_path)?;

    if entries.is_empty() {
        println!("# No history entries found.");
        return Ok(HistoryBrowserResult::Cancelled);
    }

    let mut browser = HistoryBrowser::new(entries, mode, db_path.to_path_buf());
    browser.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_filter_parse_empty() {
        let filter = HistoryFilter::parse("");
        assert!(filter.hostname.is_none());
        assert!(filter.cwd_prefix.is_none());
        assert!(filter.exit_status.is_none());
        assert!(filter.command_pattern.is_empty());
    }

    #[test]
    fn test_history_filter_parse_command_only() {
        let filter = HistoryFilter::parse("git push");
        assert!(filter.hostname.is_none());
        assert!(filter.cwd_prefix.is_none());
        assert!(filter.exit_status.is_none());
        assert_eq!(filter.command_pattern, "git push");
    }

    #[test]
    fn test_history_filter_parse_with_hostname() {
        let filter = HistoryFilter::parse("host:myserver git push");
        assert_eq!(filter.hostname, Some("myserver".to_string()));
        assert_eq!(filter.command_pattern, "git push");
    }

    #[test]
    fn test_history_filter_parse_with_cwd() {
        let filter = HistoryFilter::parse("cwd:/home/user git");
        assert_eq!(filter.cwd_prefix, Some("/home/user".to_string()));
        assert_eq!(filter.command_pattern, "git");
    }

    #[test]
    fn test_history_filter_parse_with_exit_status() {
        let filter = HistoryFilter::parse("exit:0 make");
        assert_eq!(filter.exit_status, Some(0));
        assert_eq!(filter.command_pattern, "make");
    }

    #[test]
    fn test_history_filter_parse_multiple_filters() {
        let filter = HistoryFilter::parse("host:server cwd:/project exit:1 test");
        assert_eq!(filter.hostname, Some("server".to_string()));
        assert_eq!(filter.cwd_prefix, Some("/project".to_string()));
        assert_eq!(filter.exit_status, Some(1));
        assert_eq!(filter.command_pattern, "test");
    }

    #[test]
    fn test_history_filter_parse_invalid_exit_status() {
        let filter = HistoryFilter::parse("exit:abc git");
        assert!(filter.exit_status.is_none());
        // Invalid exit:abc becomes part of command pattern
        assert_eq!(filter.command_pattern, "exit:abc git");
    }

    #[test]
    fn test_truncate_to_width() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
        assert_eq!(truncate_to_width("hello world", 8), "hello w…");
        assert_eq!(truncate_to_width("hi", 1), "…");
    }

    #[test]
    fn test_exceeds_width() {
        assert!(!exceeds_width("hello", 10));
        assert!(exceeds_width("hello world", 8));
    }

    #[test]
    fn test_scroll_display() {
        // Text that fits
        let (result, max) = scroll_display("hello", 10, 0);
        assert_eq!(result, "hello");
        assert_eq!(max, 0);

        // Text at start
        let (result, _) = scroll_display("hello world", 8, 0);
        assert_eq!(result, "hello w…");

        // Text at end
        let (result, _) = scroll_display("hello world", 8, 100);
        assert_eq!(result, "…o world");
    }

    #[test]
    fn test_db_mode_display_name() {
        assert_eq!(HistoryDbMode::R.display_name(), "R");
        assert_eq!(HistoryDbMode::Shell.display_name(), "Shell");
    }

    /// Create a temporary history database with test entries.
    /// Returns the temp dir (must be kept alive) and the db path.
    ///
    /// NOTE: The schema here must match reedline's `SqliteBackedHistory` table
    /// definition. If reedline changes its schema, this helper must be updated.
    fn create_test_db(entries: &[(&str, Option<&str>)]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test_history.db");
        let db = Connection::open(&db_path).unwrap();
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                command_line TEXT NOT NULL,
                start_timestamp INTEGER,
                session_id INTEGER,
                hostname TEXT,
                cwd TEXT,
                duration_ms INTEGER,
                exit_status INTEGER,
                more_info TEXT
            ) STRICT;",
        )
        .unwrap();
        for (cmd, hostname) in entries {
            db.execute(
                "INSERT INTO history (command_line, hostname) VALUES (?, ?)",
                rusqlite::params![cmd, hostname],
            )
            .unwrap();
        }
        (dir, db_path)
    }

    #[test]
    fn test_load_history_returns_entries_in_desc_order() {
        let (_dir, db_path) = create_test_db(&[
            ("first_cmd", Some("host1")),
            ("second_cmd", Some("host2")),
            ("third_cmd", None),
        ]);

        let items = load_history(&db_path).unwrap();
        assert_eq!(items.len(), 3);
        // Descending order by id
        assert_eq!(items[0].command_line, "third_cmd");
        assert_eq!(items[1].command_line, "second_cmd");
        assert_eq!(items[2].command_line, "first_cmd");
        // Hostname preserved
        assert_eq!(items[1].hostname.as_deref(), Some("host2"));
        assert!(items[0].hostname.is_none());
    }

    #[test]
    fn test_load_history_empty_db() {
        let (_dir, db_path) = create_test_db(&[]);
        let items = load_history(&db_path).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_delete_selected_removes_from_db_and_entries() {
        let (_dir, db_path) = create_test_db(&[("cmd_a", None), ("cmd_b", None), ("cmd_c", None)]);

        let entries = load_history(&db_path).unwrap();
        let mut browser = HistoryBrowser::new(entries, HistoryDbMode::R, db_path.clone());

        // Select the first item (cmd_c, id=3) and the third item (cmd_a, id=1)
        browser.cursor = 0;
        browser.toggle_selection();
        browser.cursor = 2;
        browser.toggle_selection();
        assert_eq!(browser.selected_count(), 2);

        browser.delete_selected().unwrap();

        // Only cmd_b should remain in the browser
        assert_eq!(browser.entries.len(), 1);
        assert_eq!(browser.entries[0].item.command_line, "cmd_b");
        assert_eq!(browser.selected_count(), 0);

        // Verify database state
        let remaining = load_history(&db_path).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].command_line, "cmd_b");
    }

    #[test]
    fn test_delete_selected_no_selection_is_noop() {
        let (_dir, db_path) = create_test_db(&[("cmd_a", None)]);
        let entries = load_history(&db_path).unwrap();
        let mut browser = HistoryBrowser::new(entries, HistoryDbMode::R, db_path.clone());

        browser.delete_selected().unwrap();

        assert_eq!(browser.entries.len(), 1);
        let remaining = load_history(&db_path).unwrap();
        assert_eq!(remaining.len(), 1);
    }

    #[test]
    fn test_delete_selected_all_entries() {
        let (_dir, db_path) = create_test_db(&[("cmd_a", None), ("cmd_b", None)]);
        let entries = load_history(&db_path).unwrap();
        let mut browser = HistoryBrowser::new(entries, HistoryDbMode::R, db_path.clone());

        browser.select_all_visible();
        assert_eq!(browser.selected_count(), 2);

        browser.delete_selected().unwrap();

        assert!(browser.entries.is_empty());
        assert!(browser.filtered.is_empty());
        let remaining = load_history(&db_path).unwrap();
        assert!(remaining.is_empty());
    }

    #[test]
    fn test_flatten_multiline() {
        // Single line - unchanged
        assert_eq!(flatten_multiline("hello"), "hello");
        assert_eq!(flatten_multiline("git push"), "git push");

        // Multiline - newlines replaced with marker
        assert_eq!(flatten_multiline("line1\nline2"), "line1↵line2");
        assert_eq!(
            flatten_multiline("function() {\n  print(1)\n}"),
            "function() {↵  print(1)↵}"
        );

        // Empty string
        assert_eq!(flatten_multiline(""), "");
    }
}
