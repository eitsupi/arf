//! Drawing and layout for the history browser.

use super::HistoryBrowser;
use crate::pager::text_utils::{
    display_width, exceeds_width, pad_to_width, scroll_display, truncate_to_width,
};
use crate::pager::{self, MinimumSize, render_size_warning_if_too_small};
use crossterm::{
    ExecutableCommand, cursor, queue,
    style::Stylize,
    terminal::{self, BeginSynchronizedUpdate, ClearType, EndSynchronizedUpdate},
};
use std::io::{self, Write};

/// Minimum terminal size for the history browser.
///
/// Width: at 70 columns the column layout (prefix 29 + cmd 20 + cwd + host + spacing)
/// fits without overflow. Below that, columns overlap.
/// Height: 7 lines of chrome + 3 minimum content rows = 10.
pub(super) const MIN_SIZE: MinimumSize = MinimumSize { cols: 70, rows: 10 };

impl HistoryBrowser {
    pub(super) fn render(&self, stdout: &mut io::Stdout) -> io::Result<()> {
        if render_size_warning_if_too_small(stdout, &MIN_SIZE)? {
            return Ok(());
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
        // The footer is the last screen row; a trailing newline would scroll
        // the whole frame up and push the header off the screen.
        if self.show_delete_dialog {
            let dialog_msg = format!(
                "  Delete {} selected entries? (Enter=confirm, Esc=cancel)",
                selected_count
            );
            print!("\r{}", pad_to_width(&dialog_msg, width).yellow().bold());
        } else if let Some(ref msg) = self.feedback_message {
            print!("\r{}", pad_to_width(&format!("  {}", msg), width));
        } else {
            let footer = if self.filter_active {
                "  Enter confirm | Esc clear | ↑↓/PgUp/PgDn navigate | Tab select"
            } else {
                "  / filter | Space/Tab select | d delete | y copy | Enter copy+exit | q exit"
            };
            print!("\r{}", pad_to_width(footer, width).dark_grey());
        }

        queue!(stdout, EndSynchronizedUpdate)?;
        stdout.flush()?;
        Ok(())
    }
}

/// Calculate layout widths for the history browser columns.
///
/// Returns (cmd_width, cwd_width, host_width).
pub(super) fn calculate_layout(cols: usize) -> (usize, usize, usize) {
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

/// Lines reserved for UI chrome: header, filter, separator, column headers,
/// footer separator and two footer lines.
const CHROME_ROWS: usize = 7;

/// Calculate the number of visible result rows.
pub(super) fn visible_result_rows() -> usize {
    pager::visible_result_rows(CHROME_ROWS)
}

/// Convert a multiline string to a single line for display.
/// Replaces newlines with a visible marker (↵) to indicate line breaks.
pub(super) fn flatten_multiline(s: &str) -> String {
    if s.contains('\n') {
        s.replace('\n', "↵")
    } else {
        s.to_string()
    }
}
