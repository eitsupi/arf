//! Help browser terminal layout and drawing.

#[cfg(test)]
mod tests;

use super::super::MIN_SIZE;
use super::HelpBrowser;
use crate::pager::text_utils::{
    display_width, exceeds_width, pad_to_width, scroll_display, truncate_to_width,
};
use crate::pager::{check_terminal_too_small, render_size_warning};
use crossterm::{
    ExecutableCommand, cursor, queue,
    style::Stylize,
    terminal::{self, BeginSynchronizedUpdate, EndSynchronizedUpdate},
};
use std::io::{self, Write};

/// Calculate layout widths for the help browser display.
/// Returns (name_width, title_width) based on terminal columns.
pub(super) fn calculate_layout(cols: usize) -> (usize, usize) {
    let prefix_width = 3; // " > " or "   "
    let spacing = 1; // space between name and title
    let name_width = (cols / 3).max(20); // ~1/3 of screen for name, min 20
    let title_width = cols.saturating_sub(prefix_width + name_width + spacing + 1);
    (name_width, title_width)
}

/// Calculate the number of visible result rows based on terminal height.
/// Layout: header(1) + filter(1) + separator(1) + results(N) + separator(1) + footer(1)
pub(super) fn visible_result_rows() -> usize {
    let (_, rows) = terminal::size().unwrap_or((80, 24));
    visible_result_rows_for(rows as usize)
}

pub(super) fn visible_result_rows_for(rows: usize) -> usize {
    // Reserve 5 lines for UI chrome (header, filter, 2 separators, footer)
    rows.saturating_sub(5).max(3)
}

impl HelpBrowser {
    /// Update the text scroll animation state.
    pub(super) fn update_text_scroll(&mut self) -> bool {
        self.text_scroll.update(self.selected)
    }

    pub(super) fn render(&self, stdout: &mut io::Stdout) -> io::Result<()> {
        if let Some((cols, rows)) = check_terminal_too_small(&MIN_SIZE) {
            return render_size_warning(stdout, cols, rows, &MIN_SIZE);
        }
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        self.render_to(stdout, cols as usize, rows as usize)
    }

    pub(super) fn render_to<W: Write>(
        &self,
        stdout: &mut W,
        width: usize,
        rows: usize,
    ) -> io::Result<()> {
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
