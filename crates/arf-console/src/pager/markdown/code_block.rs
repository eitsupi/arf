//! Code block rendering for the Markdown writer.

use super::style::{is_r_language, token_type_to_style};
use super::writer::Writer;
use crate::highlighter::tokenize_r;
use pulldown_cmark::Event;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

impl<'a, I: Iterator<Item = Event<'a>>> Writer<'a, I> {
    /// Flush the accumulated code block buffer, applying syntax highlighting
    /// for R code blocks.
    pub(super) fn flush_code_block(&mut self) {
        let buffer = std::mem::take(&mut self.code_block_buffer);
        // Remove trailing newline that pulldown-cmark typically appends
        let source = buffer.strip_suffix('\n').unwrap_or(&buffer);

        let use_r_highlight = self.code_block_lang.as_deref().is_some_and(is_r_language);

        if use_r_highlight {
            let tokens = tokenize_r(source);
            self.emit_prefix_if_needed();
            for token in &tokens {
                debug_assert!(
                    token.start <= source.len() && token.end <= source.len(),
                    "token [{}, {}) out of bounds for source len {}",
                    token.start,
                    token.end,
                    source.len()
                );
                let text = &source[token.start..token.end];
                let style = token_type_to_style(token.token_type);
                // Handle newlines within token text (whitespace tokens may span lines)
                let parts: Vec<&str> = text.split('\n').collect();
                for (i, part) in parts.iter().enumerate() {
                    if i > 0 {
                        self.flush_code_line();
                        self.emit_prefix_if_needed();
                    }
                    if !part.is_empty() {
                        self.push_span(Span::styled(part.to_string(), style));
                    }
                }
            }
            self.flush_code_line();
        } else {
            // Non-R code blocks: render with dim style
            for line_text in source.split('\n') {
                self.emit_prefix_if_needed();
                self.push_span(Span::styled(line_text.to_string(), self.styles.code_block));
                self.flush_code_line();
            }
        }
    }

    /// Like `flush_line`, but applies the code block background to the entire line.
    /// Unlike `flush_line`, this always emits a line even when spans are empty,
    /// so that blank lines within code blocks retain the background color.
    fn flush_code_line(&mut self) {
        let spans = std::mem::take(&mut self.current_spans);
        let text_index = self.push_search_text(&spans);
        let spans = spans.into_iter().map(|span| span.span).collect::<Vec<_>>();
        let line = Line::from(spans).style(Style::new().bg(self.styles.code_block_bg));
        self.push_line_with_text(line, text_index);
        self.has_output = true;
    }
}
