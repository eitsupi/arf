//! Minimum terminal size checks and the "terminal too small" warning screen.

use super::text_utils;
use crossterm::{
    cursor, queue,
    terminal::{self, BeginSynchronizedUpdate, ClearType, EndSynchronizedUpdate},
};
use std::io::{self, Write};

/// Minimum terminal size requirements for a browser UI.
pub(crate) struct MinimumSize {
    pub cols: u16,
    pub rows: u16,
}

/// Render a "terminal too small" warning screen.
///
/// This is a shared utility for browser UIs that require a minimum terminal size.
/// It fills the alternate screen with a centered message asking the user to resize.
///
/// Accepts the current terminal size to avoid a redundant `terminal::size()` call
/// (the caller already obtained the size for its check).
fn render_size_warning(
    stdout: &mut io::Stdout,
    cols: u16,
    rows: u16,
    min: &MinimumSize,
) -> io::Result<()> {
    let width = cols as usize;
    let height = rows as usize;

    let title = "Terminal too small";
    let current_line = format!("Current:  {}x{}", cols, rows);
    let minimum_line = format!("Minimum:  {}x{}", min.cols, min.rows);

    let messages = [
        title,
        "",
        &current_line,
        &minimum_line,
        "",
        "Please resize your terminal.",
        "",
        "Press q or Esc to exit.",
    ];

    queue!(
        stdout,
        BeginSynchronizedUpdate,
        cursor::MoveTo(0, 0),
        cursor::Hide
    )?;

    // Center vertically
    let start_row = height.saturating_sub(messages.len()) / 2;

    for row in 0..height {
        queue!(stdout, terminal::Clear(ClearType::CurrentLine))?;
        if row >= start_row && row < start_row + messages.len() {
            let msg = messages[row - start_row];
            let msg_width = text_utils::display_width(msg);
            let padding = width.saturating_sub(msg_width) / 2;
            if msg == title {
                write!(
                    stdout,
                    "\r{}{}",
                    " ".repeat(padding),
                    crossterm::style::Stylize::bold(crossterm::style::Stylize::yellow(msg))
                )?;
            } else {
                write!(stdout, "\r{}{}", " ".repeat(padding), msg)?;
            }
        }
        // Move to the next line (except for the last row to avoid scrolling)
        if row + 1 < height {
            write!(stdout, "\r\n")?;
        }
    }

    queue!(stdout, EndSynchronizedUpdate)?;
    stdout.flush()?;
    Ok(())
}

/// Check whether the terminal meets minimum size requirements.
///
/// Returns `Some((cols, rows))` if the terminal is too small, `None` if it meets
/// the requirements. The returned size can be passed to [`render_size_warning`]
/// to avoid a redundant `terminal::size()` call.
pub(crate) fn check_terminal_too_small(min: &MinimumSize) -> Option<(u16, u16)> {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    if is_below_minimum(cols, rows, min) {
        Some((cols, rows))
    } else {
        None
    }
}

/// Draw the "terminal too small" warning if the terminal is below `min`.
///
/// Returns `true` when the warning was drawn, in which case the caller should
/// skip drawing its own UI for this frame.
pub(crate) fn render_size_warning_if_too_small(
    stdout: &mut io::Stdout,
    min: &MinimumSize,
) -> io::Result<bool> {
    match check_terminal_too_small(min) {
        Some((cols, rows)) => {
            render_size_warning(stdout, cols, rows, min)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Fewest result rows a browser list ever shows.
const MIN_RESULT_ROWS: usize = 3;

/// Number of list rows that fit in a terminal of `rows` rows when
/// `chrome_rows` rows are used by headers, separators and footers.
pub(crate) fn visible_result_rows_for(rows: usize, chrome_rows: usize) -> usize {
    rows.saturating_sub(chrome_rows).max(MIN_RESULT_ROWS)
}

/// Number of list rows that fit in the current terminal.
pub(crate) fn visible_result_rows(chrome_rows: usize) -> usize {
    let (_, rows) = terminal::size().unwrap_or((80, 24));
    visible_result_rows_for(rows as usize, chrome_rows)
}

/// Pure comparison for minimum size check (testable without a real terminal).
fn is_below_minimum(cols: u16, rows: u16, min: &MinimumSize) -> bool {
    cols < min.cols || rows < min.rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_below_minimum() {
        let min = MinimumSize { cols: 70, rows: 10 };
        for (cols, rows, expected) in [
            (80, 24, false),
            (70, 10, false), // exact boundary
            (69, 24, true),  // columns too small
            (80, 9, true),   // rows too small
            (40, 5, true),   // both too small
        ] {
            assert_eq!(
                is_below_minimum(cols, rows, &min),
                expected,
                "{cols}x{rows}"
            );
        }
    }
}
