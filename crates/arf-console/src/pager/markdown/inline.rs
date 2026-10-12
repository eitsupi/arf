//! Inline and leaf event handlers of the Markdown writer.

use super::writer::Writer;
use pulldown_cmark::{CowStr, Event};
use ratatui::text::{Line, Span};

impl<'a, I: Iterator<Item = Event<'a>>> Writer<'a, I> {
    pub(super) fn on_text(&mut self, text: CowStr<'a>) {
        if self.in_table {
            let style = self.current_style();
            self.push_span(Span::styled(text.to_string(), style));
            return;
        }

        if self.in_code_block {
            // Buffer code block text for deferred rendering at TagEnd::CodeBlock
            self.code_block_buffer.push_str(&text);
            return;
        }

        self.emit_prefix_if_needed();
        let style = self.current_style();
        self.push_span(Span::styled(text.to_string(), style));
    }

    pub(super) fn on_inline_code(&mut self, code: CowStr<'a>) {
        if !self.in_table {
            self.emit_prefix_if_needed();
        }
        self.push_span(Span::styled(
            code.to_string(),
            self.current_style().patch(self.styles.code),
        ));
    }

    pub(super) fn on_soft_break(&mut self) {
        if self.in_table {
            self.push_span(Span::styled(" ", self.current_style()));
            return;
        }
        // Treat soft break as a space within the same line
        self.push_span(Span::styled(" ", self.current_style()));
    }

    pub(super) fn on_hard_break(&mut self) {
        if let Some(index) = self.link {
            self.links[index].label.push('\n');
        }
        self.flush_line();
    }

    pub(super) fn on_html(&mut self, html: CowStr<'a>) {
        // Block-level HTML: render non-empty lines as plain text
        for line in html.lines() {
            if !line.trim().is_empty() {
                self.emit_prefix_if_needed();
                self.push_span(Span::raw(line.to_string()));
                self.flush_line();
            }
        }
    }

    pub(super) fn on_inline_html(&mut self, html: CowStr<'a>) {
        // Detect <br>, <br/>, <br /> tags and treat as line break
        let trimmed = html.trim();
        if trimmed.eq_ignore_ascii_case("<br>")
            || trimmed.eq_ignore_ascii_case("<br/>")
            || trimmed.eq_ignore_ascii_case("<br />")
        {
            if self.in_table {
                // In a table cell, <br> separates sub-lines.
                // We use a newline character that render_table will split on.
                self.push_span(Span::raw("\n"));
            } else {
                self.on_hard_break();
            }
            return;
        }
        // Other inline HTML: render as text
        self.on_text(html);
    }

    pub(super) fn on_rule(&mut self) {
        self.flush_line();
        if self.has_output {
            self.push_blank_line();
        }
        self.push_unwrapped_line(Line::from("———"));
        self.has_output = true;
        self.needs_newline = true;
    }
}
