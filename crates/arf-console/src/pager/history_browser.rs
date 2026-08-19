//! Interactive history browser for viewing and managing command history.
//!
//! This module provides a terminal-based browser for viewing, filtering,
//! and batch-deleting command history entries stored in SQLite.

use super::copy_to_clipboard;
use super::text_utils::{
    display_width, exceeds_width, pad_to_width, scroll_display, truncate_to_width,
};
use super::{
    MinimumSize, TextScrollState, check_terminal_too_small, render_size_warning,
    with_alternate_screen,
};
use crate::fuzzy::fuzzy_match;
use crate::history::HistoryStore;
use crossterm::{
    ExecutableCommand, cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind},
    queue,
    style::Stylize,
    terminal::{self, BeginSynchronizedUpdate, ClearType, EndSynchronizedUpdate},
};
use reedline::{HistoryItem, HistoryItemId};
use std::io::{self, Write};
use std::time::Duration;

/// Maximum number of history entries to load from database.
const MAX_ENTRIES: i64 = 10000;

/// Minimum terminal size for the history browser.
///
/// Width: at 70 columns the column layout (prefix 29 + cmd 20 + cwd + host + spacing)
/// fits without overflow. Below that, columns overlap.
/// Height: 7 lines of chrome + 3 minimum content rows = 10.
const MIN_SIZE: MinimumSize = MinimumSize { cols: 70, rows: 10 };

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
    /// The already-open arf-owned history store.
    store: HistoryStore,
    /// Scroll animation state for the selected item's long text.
    text_scroll: TextScrollState,
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
    fn new(entries: Vec<HistoryItem>, db_mode: HistoryDbMode, store: HistoryStore) -> Self {
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
            store,
            text_scroll: TextScrollState::new(),
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
                        let item_host = entry.item.hostname.as_deref()?;
                        if !item_host.contains(hostname) {
                            return None;
                        }
                    }

                    // Apply cwd prefix filter
                    if let Some(ref cwd_prefix) = self.filter.cwd_prefix {
                        let item_cwd = entry.item.cwd.as_deref()?;
                        if !item_cwd.starts_with(cwd_prefix) {
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
                results.sort_by_key(|entry| std::cmp::Reverse(entry.1));
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
    /// Deletes through the already-open arf-owned store. The browser owns no
    /// database connection and never holds the store lock during UI input.
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

        let ids: Vec<HistoryItemId> = ids_to_delete
            .iter()
            .copied()
            .map(HistoryItemId::new)
            .collect();
        if let Err(error) = self.store.delete_many(&ids) {
            // A backend error can occur after a prefix of ids was deleted.
            // Reload from the owner before returning so the UI cannot retain
            // rows that no longer exist (or hide rows that were not deleted).
            self.entries = load_history(&self.store)?
                .into_iter()
                .map(|item| BrowsableHistoryItem {
                    item,
                    selected: false,
                })
                .collect();
            self.cached_selected_count = 0;
            self.update_filter();
            return Err(io::Error::other(error));
        }

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
        with_alternate_screen(|| self.run_inner())
    }

    fn run_inner(&mut self) -> io::Result<HistoryBrowserResult> {
        let mut stdout = io::stdout();
        let poll_timeout = Duration::from_millis(50);
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

            if event::poll(poll_timeout)? {
                let ev = event::read()?;
                log::trace!("history_browser: received event: {:?}", ev);
                match ev {
                    Event::Key(key) => {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }

                        needs_redraw = true;

                        // When the terminal is too small, only accept exit keys
                        // to prevent input from leaking into filter or other state.
                        if too_small {
                            match (key.code, key.modifiers) {
                                (KeyCode::Esc, _)
                                | (KeyCode::Char('q'), KeyModifiers::NONE)
                                | (KeyCode::Char('c'), KeyModifiers::CONTROL)
                                | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                                    return Ok(HistoryBrowserResult::Cancelled);
                                }
                                _ => continue,
                            }
                        }

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
                                (KeyCode::Left, KeyModifiers::NONE)
                                    if self.filter.cursor_pos > 0 =>
                                {
                                    self.filter.cursor_pos -= 1;
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
        self.text_scroll.update(self.cursor)
    }

    fn render(&self, stdout: &mut io::Stdout) -> io::Result<()> {
        if let Some((cols, rows)) = check_terminal_too_small(&MIN_SIZE) {
            return render_size_warning(stdout, cols, rows, &MIN_SIZE);
        }

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
        let (cmd_width, cwd_width, host_width) = calculate_layout(width);
        let visible_rows = visible_result_rows();

        // Column headers
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        let col_headers = format!(
            "       {:<16} {:>4} {} {} {}",
            "Date",
            "Exit",
            pad_to_width("Command", cmd_width),
            pad_to_width("Directory", cwd_width),
            pad_to_width("Host", host_width),
        );
        println!("\r{}", pad_to_width(&col_headers, width).dark_grey());

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

                // Format timestamp in local time
                let timestamp = entry
                    .item
                    .start_timestamp
                    .map(|ts| {
                        ts.with_timezone(&chrono::Local)
                            .format("%Y-%m-%d %H:%M")
                            .to_string()
                    })
                    .unwrap_or_else(|| "                ".to_string());

                // Exit status
                let exit_str = match entry.item.exit_status {
                    Some(code) => format!("{:>4}", code),
                    None => "   -".to_string(),
                };

                // Command text with scrolling for selected item
                // Convert multiline commands to single line for display
                let cmd = flatten_multiline(&entry.item.command_line);
                let display_cmd = if is_current && exceeds_width(&cmd, cmd_width) {
                    let (scrolled, _) =
                        scroll_display(&cmd, cmd_width, self.text_scroll.scroll_pos);
                    scrolled
                } else {
                    truncate_to_width(&cmd, cmd_width)
                };

                // CWD (basename only, with scrolling for current row)
                let cwd_full = entry.item.cwd.as_deref().unwrap_or("");
                let cwd_short = std::path::Path::new(cwd_full)
                    .file_name()
                    .and_then(|f| f.to_str())
                    .unwrap_or(cwd_full);
                let display_cwd = if is_current && exceeds_width(cwd_short, cwd_width) {
                    let (scrolled, _) =
                        scroll_display(cwd_short, cwd_width, self.text_scroll.scroll_pos);
                    scrolled
                } else {
                    truncate_to_width(cwd_short, cwd_width)
                };

                // Hostname (truncated)
                let host = entry.item.hostname.as_deref().unwrap_or("");
                let display_host = truncate_to_width(host, host_width);

                // Build prefix base (cursor + checkbox + timestamp, without exit)
                let prefix_base = format!("{}{} {} ", cursor_marker, checkbox, timestamp);
                let padded_cmd = pad_to_width(&display_cmd, cmd_width);
                let padded_cwd = pad_to_width(&display_cwd, cwd_width);

                if is_current {
                    let content = format!(
                        "{}{} {} {} {}",
                        prefix_base, exit_str, padded_cmd, padded_cwd, display_host
                    );
                    let line = pad_to_width(&content, width);
                    println!("\r{}", line.reverse());
                } else if entry.selected {
                    let content = format!(
                        "{}{} {} {} {}",
                        prefix_base, exit_str, padded_cmd, padded_cwd, display_host
                    );
                    let line = pad_to_width(&content, width);
                    println!("\r{}", line.yellow());
                } else {
                    // Style exit status red if non-zero, cwd and hostname dark grey
                    let styled_exit = if matches!(entry.item.exit_status, Some(code) if code != 0) {
                        format!("{}", exit_str.as_str().red())
                    } else {
                        exit_str.clone()
                    };
                    // Padding: prefix_base, exit, cmd, cwd are fixed-width (no ANSI);
                    // only hostname may be shorter than its allocated width.
                    let content_width = display_width(&prefix_base)
                        + 4
                        + 1
                        + cmd_width
                        + 1
                        + cwd_width
                        + 1
                        + display_width(&display_host);
                    let padding_len = width.saturating_sub(content_width);
                    print!(
                        "\r{}{} {} {} {}{}\n",
                        prefix_base,
                        styled_exit,
                        padded_cmd,
                        padded_cwd.dark_grey(),
                        display_host.dark_grey(),
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

/// Load history entries through the already-open arf-owned store.
///
/// The browser keeps no database lock while its UI loop is running.
fn load_history(store: &HistoryStore) -> io::Result<Vec<HistoryItem>> {
    let mut query = reedline::SearchQuery::everything(reedline::SearchDirection::Backward, None);
    query.limit = Some(MAX_ENTRIES);
    store.search(query).map_err(io::Error::other)
}

/// Calculate layout widths for the history browser columns.
///
/// Returns (cmd_width, cwd_width, host_width).
fn calculate_layout(cols: usize) -> (usize, usize, usize) {
    // Layout: " > [x] 2024-01-15 14:32    0 command...  /path/to/dir  hostname"
    // Prefix: cursor(3) + checkbox(3) + space(1) + timestamp(16) + space(1) + exit(4) + space(1) = 29
    let prefix_width = 29;
    let host_width = (cols / 8).clamp(5, 15);
    let cwd_width = (cols / 6).clamp(8, 20);
    let cmd_width = cols
        .saturating_sub(prefix_width + cwd_width + host_width + 2)
        .max(20);
    (cmd_width, cwd_width, host_width)
}

/// Calculate the number of visible result rows.
fn visible_result_rows() -> usize {
    let (_, rows) = terminal::size().unwrap_or((80, 24));
    // Reserve: header(1) + filter(1) + separator(1) + column_headers(1) + footer_separator(1) + footer(2) = 7
    rows.saturating_sub(7).max(3) as usize
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
/// * `store` - The already-open arf-owned history store
/// * `mode` - The database mode (R or Shell)
///
/// # Returns
/// The result of the browser interaction.
pub fn run_history_browser(
    store: &HistoryStore,
    mode: HistoryDbMode,
) -> io::Result<HistoryBrowserResult> {
    // Load history entries
    let entries = load_history(store)?;

    if entries.is_empty() {
        println!("# No history entries found.");
        return Ok(HistoryBrowserResult::Cancelled);
    }

    let mut browser = HistoryBrowser::new(entries, mode, store.clone());
    browser.run()
}

#[cfg(test)]
mod tests;
