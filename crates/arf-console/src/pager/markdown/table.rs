//! Table rendering for the Markdown writer.

use super::writer::Writer;
use super::{InlineSpan, TextFragment};
use crate::pager::text_utils::{WrappedLine, wrap_spans_with_ranges};
use pulldown_cmark::Event;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

struct TableLine {
    text_index: usize,
    wrapped: WrappedLine,
}

impl<'a, I: Iterator<Item = Event<'a>>> Writer<'a, I> {
    pub(super) fn render_table(&mut self) {
        if self.table_rows.is_empty() {
            return;
        }

        let n_cols = self.table_rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if n_cols == 0 {
            return;
        }

        // Pre-split all cells into sub-lines
        let mut split_rows: Vec<Vec<Vec<TableLine>>> = Vec::new();
        for row in std::mem::take(&mut self.table_rows) {
            let mut cells = Vec::new();
            for cell in row {
                let mut cell_lines = Vec::new();
                for spans in split_cell_lines(&cell) {
                    let text_index = self.push_search_text(&spans);
                    let spans = spans.into_iter().map(|span| span.span).collect();
                    cell_lines.push(TableLine {
                        text_index,
                        wrapped: WrappedLine::unwrapped(spans),
                    });
                }
                cells.push(cell_lines);
            }
            split_rows.push(cells);
        }

        // Calculate column widths (max sub-line width across all rows)
        let mut col_widths = vec![0usize; n_cols];
        for row in &split_rows {
            for (col_idx, cell_lines) in row.iter().enumerate() {
                for sub_line in cell_lines {
                    let w = spans_width(&sub_line.wrapped.spans);
                    col_widths[col_idx] = col_widths[col_idx].max(w);
                }
            }
        }

        // If wrap_width is set and the table is too wide, constrain column
        // widths and re-wrap cell content.
        if let Some(wrap_width) = self.wrap_width {
            // Table decoration: "| " prefix (2) + " | " after each column (3 × n_cols)
            // + blockquote prefix (2 per depth level)
            let decoration = self.blockquote_depth * 2 + 2 + n_cols * 3;
            let available = wrap_width.saturating_sub(decoration);
            let total_natural: usize = col_widths.iter().sum();

            if total_natural > available && available > 0 {
                let target_widths = distribute_column_widths(&col_widths, available);

                // Re-wrap cells that exceed their target width
                for row in &mut split_rows {
                    for (col_idx, cell_lines) in row.iter_mut().enumerate() {
                        let target = target_widths[col_idx];
                        if target == 0 {
                            continue;
                        }
                        let mut new_lines = Vec::new();
                        for sub_line in cell_lines.drain(..) {
                            for wrapped in
                                wrap_spans_with_ranges(&sub_line.wrapped.spans, target, 0)
                            {
                                new_lines.push(TableLine {
                                    text_index: sub_line.text_index,
                                    wrapped,
                                });
                            }
                        }
                        *cell_lines = new_lines;
                    }
                }

                col_widths = target_widths;
            }
        }

        // Render each row (potentially multiple visual lines)
        for (row_idx, row) in split_rows.iter().enumerate() {
            let max_sub_lines = row.iter().map(|cell| cell.len()).max().unwrap_or(1);

            for sub_line_idx in 0..max_sub_lines {
                let mut spans: Vec<Span<'static>> = Vec::new();

                for _ in 0..self.blockquote_depth {
                    spans.push(Span::styled("> ", self.styles.blockquote_prefix));
                }

                spans.push(Span::raw("| "));
                for (col_idx, cell_lines) in row.iter().enumerate() {
                    let target = col_widths.get(col_idx).copied().unwrap_or(0);

                    if let Some(sub_line) = cell_lines.get(sub_line_idx) {
                        let w = spans_width(&sub_line.wrapped.spans);
                        let visual_start =
                            spans.iter().map(|span| span.content.len()).sum::<usize>();
                        self.text[sub_line.text_index].fragments.push(TextFragment {
                            line: self.lines.len(),
                            source_range: sub_line.wrapped.source_range.clone(),
                            visual_start: visual_start + sub_line.wrapped.indent,
                        });
                        for s in &sub_line.wrapped.spans {
                            spans.push(s.clone());
                        }
                        let pad = target.saturating_sub(w);
                        if pad > 0 {
                            spans.push(Span::raw(" ".repeat(pad)));
                        }
                    } else {
                        // Empty sub-line for this cell
                        spans.push(Span::raw(" ".repeat(target)));
                    }
                    spans.push(Span::raw(" | "));
                }
                self.lines.push(Line::from(spans));
                self.has_output = true;
            }

            // Separator after header row
            if row_idx == 0 {
                let mut sep_spans: Vec<Span<'static>> = Vec::new();
                for _ in 0..self.blockquote_depth {
                    sep_spans.push(Span::styled("> ", self.styles.blockquote_prefix));
                }
                sep_spans.push(Span::raw("| "));
                for (col_idx, w) in col_widths.iter().enumerate() {
                    sep_spans.push(Span::raw("-".repeat(*w)));
                    if col_idx + 1 < n_cols {
                        sep_spans.push(Span::raw(" | "));
                    }
                }
                sep_spans.push(Span::raw(" |"));
                self.lines.push(Line::from(sep_spans));
            }
        }
    }
}

/// Split a cell's spans into sub-lines at `\n` boundaries (from `<br>` tags).
fn split_cell_lines(cell: &[InlineSpan]) -> Vec<Vec<InlineSpan>> {
    let mut lines: Vec<Vec<InlineSpan>> = vec![Vec::new()];
    for span in cell {
        if span.span.content.as_ref() == "\n" {
            lines.push(Vec::new());
        } else {
            lines.last_mut().unwrap().push(span.clone());
        }
    }
    lines
}

/// Calculate the display width of a list of spans using Unicode width.
fn spans_width(spans: &[Span<'static>]) -> usize {
    spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum()
}

/// Distribute `available` columns among table columns.
///
/// Small columns (those that fit within a fair share) keep their natural
/// width; remaining space is divided equally among wider columns.
/// Every column gets at least 1 column of width.
pub(super) fn distribute_column_widths(natural: &[usize], available: usize) -> Vec<usize> {
    let n = natural.len();
    if n == 0 {
        return vec![];
    }

    // Ensure every column gets at least 1.  When available < n, give
    // 1 to the first `available` columns and 0 to the rest so that the
    // sum never exceeds `available`.
    if available <= n {
        let mut widths = vec![0usize; n];
        for w in widths.iter_mut().take(available) {
            *w = 1;
        }
        return widths;
    }

    let mut result = natural.to_vec();
    let mut locked = vec![false; n];
    let mut locked_total = 0usize;
    let mut unlocked_count = n;

    // Iteratively lock columns whose natural width fits in the fair share.
    loop {
        let remaining = available.saturating_sub(locked_total);
        let share = remaining.checked_div(unlocked_count).unwrap_or(0);

        let mut changed = false;
        for i in 0..n {
            if !locked[i] && result[i] <= share {
                locked[i] = true;
                locked_total += result[i];
                unlocked_count -= 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Distribute remaining space among unlocked (wide) columns.
    if unlocked_count > 0 {
        let remaining = available.saturating_sub(locked_total);
        let share = remaining.checked_div(unlocked_count).unwrap_or(0);
        let extra = remaining % unlocked_count;
        let mut idx = 0;
        for i in 0..n {
            if !locked[i] {
                result[i] = share + if idx < extra { 1 } else { 0 };
                idx += 1;
            }
        }
    }

    result
}
