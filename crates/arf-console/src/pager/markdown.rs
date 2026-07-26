//! Markdown to ratatui `Line` renderer.
//!
//! Converts CommonMark text into styled `Vec<Line<'static>>` suitable for
//! the pager. No width-aware wrapping is performed — each logical line maps
//! to exactly one `Line`.
//!
//! Reference: `refs/codex/codex-rs/tui2/src/markdown_render.rs`

use pulldown_cmark::{CodeBlockKind, CowStr, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use crate::config::RColorConfig;
use crate::highlighter::{TokenType, tokenize_r};
use crate::pager::style_convert::nu_ansi_color_to_ratatui;

/// Render a Markdown string into styled ratatui lines.
///
/// `default_code_lang` is used for fenced code blocks that have no language tag.
/// Pass `Some("r")` when rendering R documentation (help pages, vignettes) so
/// that untagged code blocks receive R syntax highlighting.
///
/// `wrap_width` enables word-wrapping for prose content (paragraphs,
/// blockquotes, list items).  Code blocks, tables, and headings are never
/// wrapped.  Pass `None` to disable wrapping (every logical line maps to
/// exactly one `Line`).
pub fn render_markdown(
    input: &str,
    default_code_lang: Option<&str>,
    wrap_width: Option<usize>,
) -> Vec<Line<'static>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(input, options);
    let mut writer = Writer::new(parser, default_code_lang.map(|s| s.to_string()), wrap_width);
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
    /// Background color applied to entire code block lines (via `Line::style`).
    code_block_bg: Color,
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
            code_block_bg: Color::Indexed(236),
        }
    }
}

// ---------------------------------------------------------------------------
// R syntax highlighting for code blocks
// ---------------------------------------------------------------------------

/// Map a `TokenType` to a `ratatui::style::Style` using default R colors.
///
/// Derives colors from [`RColorConfig::default()`] via [`nu_ansi_color_to_ratatui`]
/// so the pager stays in sync with the REPL color scheme.
fn token_type_to_style(tt: TokenType) -> Style {
    // Use a static default so we only build it once.
    static CONFIG: std::sync::LazyLock<RColorConfig> =
        std::sync::LazyLock::new(RColorConfig::default);

    let color = match tt {
        TokenType::Comment => CONFIG.comment,
        TokenType::String => CONFIG.string,
        TokenType::Number => CONFIG.number,
        TokenType::Keyword => CONFIG.keyword,
        TokenType::Constant => CONFIG.constant,
        TokenType::Operator => CONFIG.operator,
        TokenType::Punctuation => CONFIG.punctuation,
        TokenType::Identifier => CONFIG.identifier,
        TokenType::Whitespace | TokenType::Other => return Style::default(),
    };

    match color {
        nu_ansi_term::Color::Default => Style::default(),
        c => Style::new().fg(nu_ansi_color_to_ratatui(c)),
    }
}

/// Check if a code block language tag indicates R code.
fn is_r_language(lang: &str) -> bool {
    matches!(lang, "r" | "R")
}

// ---------------------------------------------------------------------------
// Writer state machine
// ---------------------------------------------------------------------------

struct Writer<'a, I: Iterator<Item = Event<'a>>> {
    iter: I,
    lines: Vec<Line<'static>>,
    styles: Styles,

    /// Default language for code blocks without a language tag.
    default_code_lang: Option<String>,

    /// If set, wrap prose lines to this width (code blocks, tables, headings
    /// are excluded from wrapping).
    wrap_width: Option<usize>,

    /// Stack of inline styles (emphasis, strong, …).
    inline_styles: Vec<Style>,

    /// Current spans being accumulated for the current line.
    current_spans: Vec<Span<'static>>,

    /// Heading level, if currently inside a heading tag.
    in_heading: Option<HeadingLevel>,

    /// Inside a code block (fenced or indented).
    in_code_block: bool,

    /// Language tag for the current code block (e.g., "r", "python").
    code_block_lang: Option<String>,

    /// Buffer accumulating code block text for deferred rendering.
    code_block_buffer: String,

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
    fn new(iter: I, default_code_lang: Option<String>, wrap_width: Option<usize>) -> Self {
        Self {
            iter,
            lines: Vec::new(),
            styles: Styles::default(),
            default_code_lang,
            wrap_width,
            inline_styles: Vec::new(),
            current_spans: Vec::new(),
            in_heading: None,
            in_code_block: false,
            code_block_lang: None,
            code_block_buffer: String::new(),
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
            Event::Html(html) => self.on_html(html),
            Event::InlineHtml(html) => self.on_inline_html(html),
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
            Tag::CodeBlock(kind) => {
                self.ensure_blank_line_before_block();
                self.in_code_block = true;
                let explicit_lang = match kind {
                    CodeBlockKind::Fenced(lang) => {
                        let lang = lang.split_whitespace().next().unwrap_or("");
                        if lang.is_empty() {
                            None
                        } else {
                            Some(lang.to_string())
                        }
                    }
                    CodeBlockKind::Indented => None,
                };
                // Fall back to the default language when no explicit tag is given
                self.code_block_lang = explicit_lang.or_else(|| self.default_code_lang.clone());
                self.code_block_buffer.clear();
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
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => {}
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
                self.flush_code_block();
                self.in_code_block = false;
                self.code_block_lang = None;
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
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
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
            // Buffer code block text for deferred rendering at TagEnd::CodeBlock
            self.code_block_buffer.push_str(&text);
            return;
        }

        self.emit_prefix_if_needed();
        let style = self.current_style();
        self.push_span(Span::styled(text.to_string(), style));
    }

    fn on_inline_code(&mut self, code: CowStr<'a>) {
        if self.in_table {
            self.table_cell_spans
                .push(Span::styled(code.to_string(), self.styles.code));
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

    fn on_html(&mut self, html: CowStr<'a>) {
        // Block-level HTML: render non-empty lines as plain text
        for line in html.lines() {
            if !line.trim().is_empty() {
                self.emit_prefix_if_needed();
                self.push_span(Span::raw(line.to_string()));
                self.flush_line();
            }
        }
    }

    fn on_inline_html(&mut self, html: CowStr<'a>) {
        // Detect <br>, <br/>, <br /> tags and treat as line break
        let trimmed = html.trim();
        if trimmed.eq_ignore_ascii_case("<br>")
            || trimmed.eq_ignore_ascii_case("<br/>")
            || trimmed.eq_ignore_ascii_case("<br />")
        {
            if self.in_table {
                // In a table cell, <br> separates sub-lines.
                // We use a newline character that render_table will split on.
                self.table_cell_spans.push(Span::raw("\n"));
            } else {
                self.flush_line();
            }
            return;
        }
        // Other inline HTML: render as text
        self.on_text(html);
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

        // Apply word-wrapping for prose content (not code blocks, tables, or headings).
        if let Some(width) = self.wrap_width
            && !self.in_code_block
            && !self.in_table
            && self.in_heading.is_none()
        {
            let indent = self.continuation_indent();
            let wrapped = super::text_utils::wrap_spans(&spans, width, indent);
            for line_spans in wrapped {
                self.lines.push(Line::from(line_spans));
            }
            self.has_output = true;
            return;
        }

        self.lines.push(Line::from(spans));
        self.has_output = true;
    }

    /// Compute the continuation indent for wrapped lines.
    ///
    /// This aligns continuation text under the content start of the
    /// current context (blockquote prefix + list indentation).
    fn continuation_indent(&self) -> usize {
        let mut indent = self.blockquote_depth * 2; // "> " per level
        if self.list_depth > 0 {
            // Nesting indent for outer levels
            indent += self.list_depth.saturating_sub(1) * 2;
            // Marker width: "- " (2) for unordered, "N. " (digits + 2) for ordered
            if let Some(Some(idx)) = self.list_indices.last() {
                // Count digits in the current index
                let digits = if *idx == 0 {
                    1
                } else {
                    (*idx).ilog10() as usize + 1
                };
                indent += digits + 2; // "N. "
            } else {
                indent += 2; // "- "
            }
        }
        indent
    }

    /// Like `flush_line`, but applies the code block background to the entire line.
    /// Unlike `flush_line`, this always emits a line even when spans are empty,
    /// so that blank lines within code blocks retain the background color.
    fn flush_code_line(&mut self) {
        let spans = std::mem::take(&mut self.current_spans);
        let line = Line::from(spans).style(Style::new().bg(self.styles.code_block_bg));
        self.lines.push(line);
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

    // -- code block rendering ------------------------------------------------

    /// Flush the accumulated code block buffer, applying syntax highlighting
    /// for R code blocks.
    fn flush_code_block(&mut self) {
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

    // -- table rendering ----------------------------------------------------

    /// Split a cell's spans into sub-lines at `\n` boundaries (from `<br>` tags).
    fn split_cell_lines(cell: &[Span<'static>]) -> Vec<Vec<Span<'static>>> {
        let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
        for span in cell {
            if span.content.as_ref() == "\n" {
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

    fn render_table(&mut self) {
        if self.table_rows.is_empty() {
            return;
        }

        let n_cols = self.table_rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if n_cols == 0 {
            return;
        }

        // Pre-split all cells into sub-lines
        let mut split_rows: Vec<Vec<Vec<Vec<Span<'static>>>>> = self
            .table_rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(|cell| Self::split_cell_lines(cell))
                    .collect()
            })
            .collect();

        // Calculate column widths (max sub-line width across all rows)
        let mut col_widths = vec![0usize; n_cols];
        for row in &split_rows {
            for (col_idx, cell_lines) in row.iter().enumerate() {
                for sub_line in cell_lines {
                    let w = Self::spans_width(sub_line);
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
                        let mut new_lines: Vec<Vec<Span<'static>>> = Vec::new();
                        for sub_line in cell_lines.drain(..) {
                            let wrapped = super::text_utils::wrap_spans(&sub_line, target, 0);
                            new_lines.extend(wrapped);
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
                        let w = Self::spans_width(sub_line);
                        for s in sub_line {
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

/// Distribute `available` columns among table columns.
///
/// Small columns (those that fit within a fair share) keep their natural
/// width; remaining space is divided equally among wider columns.
/// Every column gets at least 1 column of width.
fn distribute_column_widths(natural: &[usize], available: usize) -> Vec<usize> {
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

#[cfg(test)]
mod tests;
