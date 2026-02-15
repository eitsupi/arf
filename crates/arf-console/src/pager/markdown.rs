//! Markdown to ratatui `Line` renderer.
//!
//! Converts CommonMark text into styled `Vec<Line<'static>>` suitable for
//! the pager. No width-aware wrapping is performed — each logical line maps
//! to exactly one `Line`.
//!
//! Reference: `refs/codex/codex-rs/tui2/src/markdown_render.rs`

use pulldown_cmark::{CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

/// Render a Markdown string into styled ratatui lines.
pub fn render_markdown(input: &str) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(input, options);
    let mut writer = Writer::new(parser);
    writer.run();
    writer.lines
}

// ---------------------------------------------------------------------------
// Styles
// ---------------------------------------------------------------------------

struct Styles {
    h1: Style,
    h2: Style,
    h3: Style,
    code: Style,
    emphasis: Style,
    strong: Style,
    link: Style,
    blockquote_prefix: Style,
    code_block: Style,
}

impl Default for Styles {
    fn default() -> Self {
        Self {
            h1: Style::new().bold().underlined(),
            h2: Style::new().bold(),
            h3: Style::new().italic(),
            code: Style::new().cyan(),
            emphasis: Style::new().italic(),
            strong: Style::new().bold(),
            link: Style::new().cyan().underlined(),
            blockquote_prefix: Style::new().green(),
            code_block: Style::new().dim(),
        }
    }
}

// ---------------------------------------------------------------------------
// Writer state machine
// ---------------------------------------------------------------------------

struct Writer<'a, I: Iterator<Item = Event<'a>>> {
    iter: I,
    lines: Vec<Line<'static>>,
    styles: Styles,

    /// Stack of inline styles (emphasis, strong, …).
    inline_styles: Vec<Style>,

    /// Current spans being accumulated for the current line.
    current_spans: Vec<Span<'static>>,

    /// Heading level, if currently inside a heading tag.
    in_heading: Option<HeadingLevel>,

    /// Inside a code block (fenced or indented).
    in_code_block: bool,

    /// Track ordered list counters (Some(n) = ordered starting at n).
    list_indices: Vec<Option<u64>>,

    /// Nesting depth for list indentation.
    list_depth: usize,

    /// Blockquote nesting depth.
    blockquote_depth: usize,

    /// Link URL being collected (set on Tag::Link start).
    link: Option<String>,

    /// Whether we need a blank line before the next block element.
    needs_newline: bool,

    /// Whether the next item line needs a list marker.
    pending_marker: bool,

    /// Whether we have emitted any content yet.
    has_output: bool,

    /// Inside a table.
    in_table: bool,

    /// Collecting table rows: each row is a vec of cell-span groups.
    table_rows: Vec<Vec<Vec<Span<'static>>>>,

    /// Current row's cells being accumulated.
    table_current_row: Vec<Vec<Span<'static>>>,

    /// Spans for the current table cell.
    table_cell_spans: Vec<Span<'static>>,
}

impl<'a, I: Iterator<Item = Event<'a>>> Writer<'a, I> {
    fn new(iter: I) -> Self {
        Self {
            iter,
            lines: Vec::new(),
            styles: Styles::default(),
            inline_styles: Vec::new(),
            current_spans: Vec::new(),
            in_heading: None,
            in_code_block: false,
            list_indices: Vec::new(),
            list_depth: 0,
            blockquote_depth: 0,
            link: None,
            needs_newline: false,
            pending_marker: false,
            has_output: false,
            in_table: false,
            table_rows: Vec::new(),
            table_current_row: Vec::new(),
            table_cell_spans: Vec::new(),
        }
    }

    fn run(&mut self) {
        while let Some(ev) = self.iter.next() {
            self.handle_event(ev);
        }
        self.flush_line();
    }

    // -- event dispatch -----------------------------------------------------

    fn handle_event(&mut self, event: Event<'a>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.on_text(text),
            Event::Code(code) => self.on_inline_code(code),
            Event::SoftBreak => self.on_soft_break(),
            Event::HardBreak => self.on_hard_break(),
            Event::Rule => self.on_rule(),
            Event::Html(html) => self.on_text(html),
            Event::InlineHtml(html) => self.on_text(html),
            Event::FootnoteReference(_)
            | Event::TaskListMarker(_)
            | Event::InlineMath(_)
            | Event::DisplayMath(_) => {}
        }
    }

    // -- tag start ----------------------------------------------------------

    fn start_tag(&mut self, tag: Tag<'a>) {
        match tag {
            Tag::Paragraph => {
                if self.in_table {
                    return;
                }
                self.ensure_blank_line_before_block();
            }
            Tag::Heading { level, .. } => {
                self.ensure_blank_line_before_block();
                self.in_heading = Some(level);
                let prefix = match level {
                    HeadingLevel::H1 => "# ",
                    HeadingLevel::H2 => "## ",
                    HeadingLevel::H3 => "### ",
                    HeadingLevel::H4 => "#### ",
                    HeadingLevel::H5 => "##### ",
                    HeadingLevel::H6 => "###### ",
                };
                let style = match level {
                    HeadingLevel::H1 => self.styles.h1,
                    HeadingLevel::H2 => self.styles.h2,
                    _ => self.styles.h3,
                };
                self.push_span(Span::styled(prefix.to_string(), style));
                self.inline_styles.push(style);
            }
            Tag::BlockQuote(_) => {
                self.ensure_blank_line_before_block();
                self.blockquote_depth += 1;
            }
            Tag::CodeBlock(_kind) => {
                self.ensure_blank_line_before_block();
                self.in_code_block = true;
            }
            Tag::List(start) => {
                if self.list_depth == 0 {
                    self.ensure_blank_line_before_block();
                } else {
                    // Nested list: flush the parent item's current line
                    self.flush_line();
                }
                self.list_indices.push(start);
                self.list_depth += 1;
            }
            Tag::Item => {
                self.flush_line();
                self.pending_marker = true;
            }
            Tag::Emphasis => {
                self.inline_styles.push(self.styles.emphasis);
            }
            Tag::Strong => {
                self.inline_styles.push(self.styles.strong);
            }
            Tag::Strikethrough => {
                self.inline_styles.push(Style::new().crossed_out());
            }
            Tag::Link { dest_url, .. } => {
                self.link = Some(dest_url.to_string());
            }
            Tag::Table(_) => {
                self.ensure_blank_line_before_block();
                self.in_table = true;
                self.table_rows.clear();
            }
            Tag::TableHead => {
                self.table_current_row.clear();
            }
            Tag::TableRow => {
                self.table_current_row.clear();
            }
            Tag::TableCell => {
                self.table_cell_spans.clear();
            }
            Tag::HtmlBlock
            | Tag::Image { .. }
            | Tag::FootnoteDefinition(_)
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition => {}
        }
    }

    // -- tag end ------------------------------------------------------------

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if self.in_table {
                    return;
                }
                self.flush_line();
                self.needs_newline = true;
            }
            TagEnd::Heading(_) => {
                self.inline_styles.pop();
                self.flush_line();
                self.in_heading = None;
                self.needs_newline = true;
            }
            TagEnd::BlockQuote(_) => {
                self.flush_line();
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
                self.needs_newline = true;
            }
            TagEnd::CodeBlock => {
                self.flush_line();
                self.in_code_block = false;
                self.needs_newline = true;
            }
            TagEnd::List(_) => {
                self.flush_line();
                self.list_indices.pop();
                self.list_depth = self.list_depth.saturating_sub(1);
                if self.list_depth == 0 {
                    self.needs_newline = true;
                }
            }
            TagEnd::Item => {
                self.flush_line();
                // Increment ordered list counter
                if let Some(Some(idx)) = self.list_indices.last_mut() {
                    *idx += 1;
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                self.inline_styles.pop();
            }
            TagEnd::Link => {
                if let Some(url) = self.link.take() {
                    // Append URL after link text
                    self.push_span(Span::styled(format!(" ({})", url), self.styles.link));
                }
            }
            TagEnd::Table => {
                self.render_table();
                self.in_table = false;
                self.needs_newline = true;
            }
            TagEnd::TableHead => {
                let row = std::mem::take(&mut self.table_current_row);
                self.table_rows.insert(0, row); // header is first row
            }
            TagEnd::TableRow => {
                let row = std::mem::take(&mut self.table_current_row);
                self.table_rows.push(row);
            }
            TagEnd::TableCell => {
                let spans = std::mem::take(&mut self.table_cell_spans);
                self.table_current_row.push(spans);
            }
            TagEnd::HtmlBlock
            | TagEnd::Image
            | TagEnd::FootnoteDefinition
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => {}
        }
    }

    // -- inline events ------------------------------------------------------

    fn on_text(&mut self, text: CowStr<'a>) {
        if self.in_table {
            let style = self.current_style();
            self.table_cell_spans
                .push(Span::styled(text.to_string(), style));
            return;
        }

        if self.in_code_block {
            // Code blocks: split into lines, each gets its own Line
            let s = text.to_string();
            let mut line_iter = s.split('\n').peekable();
            while let Some(line_text) = line_iter.next() {
                self.emit_prefix_if_needed();
                self.push_span(Span::styled(line_text.to_string(), self.styles.code_block));
                if line_iter.peek().is_some() {
                    self.flush_line();
                }
            }
            return;
        }

        self.emit_prefix_if_needed();
        let style = self.current_style();
        self.push_span(Span::styled(text.to_string(), style));
    }

    fn on_inline_code(&mut self, code: CowStr<'a>) {
        if self.in_table {
            self.table_cell_spans
                .push(Span::styled(format!("`{}`", code), self.styles.code));
            return;
        }
        self.emit_prefix_if_needed();
        self.push_span(Span::styled(code.to_string(), self.styles.code));
    }

    fn on_soft_break(&mut self) {
        if self.in_table {
            self.table_cell_spans.push(Span::raw(" "));
            return;
        }
        // Treat soft break as a space within the same line
        self.push_span(Span::raw(" "));
    }

    fn on_hard_break(&mut self) {
        self.flush_line();
    }

    fn on_rule(&mut self) {
        self.flush_line();
        if self.has_output {
            self.push_blank_line();
        }
        self.lines.push(Line::from("———"));
        self.has_output = true;
        self.needs_newline = true;
    }

    // -- helpers ------------------------------------------------------------

    fn current_style(&self) -> Style {
        let mut s = Style::default();
        for sty in &self.inline_styles {
            s = s.patch(*sty);
        }
        s
    }

    fn push_span(&mut self, span: Span<'static>) {
        self.current_spans.push(span);
    }

    /// Prepend blockquote / list-marker prefix to the current line, if needed.
    fn emit_prefix_if_needed(&mut self) {
        // Only emit prefix at the start of a new line (empty current_spans)
        if !self.current_spans.is_empty() {
            return;
        }

        // Blockquote prefix
        for _ in 0..self.blockquote_depth {
            self.push_span(Span::styled("> ", self.styles.blockquote_prefix));
        }

        // List marker / indentation
        if self.list_depth > 0 {
            // Indentation for nesting (each level except the innermost)
            let indent_levels = self.list_depth.saturating_sub(1);
            if indent_levels > 0 {
                self.push_span(Span::raw("  ".repeat(indent_levels)));
            }

            if self.pending_marker {
                // Emit marker
                if let Some(maybe_idx) = self.list_indices.last() {
                    match maybe_idx {
                        Some(idx) => {
                            self.push_span(Span::raw(format!("{}. ", idx)));
                        }
                        None => {
                            self.push_span(Span::raw("- "));
                        }
                    }
                }
                self.pending_marker = false;
            } else {
                // Continuation indent (align after marker)
                self.push_span(Span::raw("  "));
            }
        }
    }

    fn flush_line(&mut self) {
        if self.current_spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.current_spans);
        self.lines.push(Line::from(spans));
        self.has_output = true;
    }

    fn push_blank_line(&mut self) {
        self.lines.push(Line::from(""));
    }

    fn ensure_blank_line_before_block(&mut self) {
        if self.needs_newline && self.has_output {
            self.flush_line();
            self.push_blank_line();
            self.needs_newline = false;
        }
    }

    // -- table rendering ----------------------------------------------------

    fn render_table(&mut self) {
        if self.table_rows.is_empty() {
            return;
        }

        // Calculate column widths
        let n_cols = self.table_rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if n_cols == 0 {
            return;
        }

        let mut col_widths = vec![0usize; n_cols];
        for row in &self.table_rows {
            for (i, cell) in row.iter().enumerate() {
                let w: usize = cell.iter().map(|s| s.content.len()).sum();
                col_widths[i] = col_widths[i].max(w);
            }
        }

        // Render each row
        for (row_idx, row) in self.table_rows.iter().enumerate() {
            let mut spans: Vec<Span<'static>> = Vec::new();

            // Blockquote prefix
            for _ in 0..self.blockquote_depth {
                spans.push(Span::styled("> ", self.styles.blockquote_prefix));
            }

            spans.push(Span::raw("| "));
            for (col_idx, cell) in row.iter().enumerate() {
                let cell_width: usize = cell.iter().map(|s| s.content.len()).sum();
                let target = col_widths.get(col_idx).copied().unwrap_or(0);

                for s in cell {
                    spans.push(s.clone());
                }
                // Pad to column width
                let pad = target.saturating_sub(cell_width);
                if pad > 0 {
                    spans.push(Span::raw(" ".repeat(pad)));
                }
                spans.push(Span::raw(" | "));
            }
            self.lines.push(Line::from(spans));
            self.has_output = true;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: render markdown and collect line text (without styling).
    fn render_plain(input: &str) -> Vec<String> {
        render_markdown(input)
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn plain_text() {
        let lines = render_plain("Hello world");
        assert_eq!(lines, vec!["Hello world"]);
    }

    #[test]
    fn heading_prefix() {
        let lines = render_plain("# Title\n\nBody");
        assert_eq!(lines, vec!["# Title", "", "Body"]);
    }

    #[test]
    fn heading_levels() {
        let lines = render_plain("## Sub\n\n### Third");
        assert_eq!(lines, vec!["## Sub", "", "### Third"]);
    }

    #[test]
    fn emphasis_and_strong() {
        let lines = render_markdown("*em* **strong**");
        assert_eq!(lines.len(), 1);
        // Check that there are separate spans with appropriate styles
        let spans = &lines[0].spans;
        assert!(spans.len() >= 2);
    }

    #[test]
    fn inline_code() {
        let lines = render_markdown("Use `print()`");
        assert_eq!(lines.len(), 1);
        let text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Use print()");
    }

    #[test]
    fn code_block() {
        let input = "```r\nx <- 1\ny <- 2\n```";
        let lines = render_plain(input);
        assert!(lines.contains(&"x <- 1".to_string()));
        assert!(lines.contains(&"y <- 2".to_string()));
    }

    #[test]
    fn unordered_list() {
        let input = "- one\n- two\n- three";
        let lines = render_plain(input);
        assert_eq!(lines, vec!["- one", "- two", "- three"]);
    }

    #[test]
    fn ordered_list() {
        let input = "1. first\n2. second\n3. third";
        let lines = render_plain(input);
        assert_eq!(lines, vec!["1. first", "2. second", "3. third"]);
    }

    #[test]
    fn nested_list() {
        let input = "- outer\n  - inner";
        let lines = render_plain(input);
        assert_eq!(lines, vec!["- outer", "  - inner"]);
    }

    #[test]
    fn blockquote() {
        let input = "> quoted text";
        let lines = render_plain(input);
        assert_eq!(lines, vec!["> quoted text"]);
    }

    #[test]
    fn simple_table() {
        let input = "| A | B |\n|---|---|\n| 1 | 2 |";
        let lines = render_plain(input);
        // Should have header, separator, data row
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("A"));
        assert!(lines[0].contains("B"));
        assert!(lines[1].contains("-"));
        assert!(lines[2].contains("1"));
        assert!(lines[2].contains("2"));
    }

    #[test]
    fn link_rendering() {
        let input = "[click here](https://example.com)";
        let lines = render_plain(input);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("click here"));
        assert!(lines[0].contains("https://example.com"));
    }

    #[test]
    fn horizontal_rule() {
        let input = "before\n\n---\n\nafter";
        let lines = render_plain(input);
        assert!(lines.contains(&"———".to_string()));
    }

    #[test]
    fn empty_input() {
        let lines = render_plain("");
        assert!(lines.is_empty());
    }

    #[test]
    fn paragraphs_separated_by_blank_line() {
        let input = "First paragraph.\n\nSecond paragraph.";
        let lines = render_plain(input);
        assert_eq!(lines, vec!["First paragraph.", "", "Second paragraph."]);
    }
}
