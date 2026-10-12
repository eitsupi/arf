//! Event-driven Markdown writer: block structure, inline spans and line flushing.

use super::style::Styles;
use super::{InlineSpan, LinkDisplay, LinkTextRange, RenderedLink, RenderedText, TextFragment};
use crate::pager::text_utils::wrap_spans_with_ranges;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Tag, TagEnd};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

pub(super) struct Writer<'a, I: Iterator<Item = Event<'a>>> {
    pub(super) iter: I,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) text: Vec<RenderedText>,
    pub(super) links: Vec<RenderedLink>,
    pub(super) link_display: &'a dyn Fn(&str) -> LinkDisplay,
    pub(super) styles: Styles,

    /// Default language for code blocks without a language tag.
    pub(super) default_code_lang: Option<String>,

    /// If set, wrap prose lines and constrain table cells to this width.
    /// Code blocks and headings retain their original line boundaries.
    pub(super) wrap_width: Option<usize>,

    /// Stack of inline styles (emphasis, strong, …).
    pub(super) inline_styles: Vec<Style>,

    /// Current spans being accumulated for the current line.
    pub(super) current_spans: Vec<InlineSpan>,

    /// Heading level, if currently inside a heading tag.
    pub(super) in_heading: Option<HeadingLevel>,

    /// Inside a code block (fenced or indented).
    pub(super) in_code_block: bool,

    /// Language tag for the current code block (e.g., "r", "python").
    pub(super) code_block_lang: Option<String>,

    /// Buffer accumulating code block text for deferred rendering.
    pub(super) code_block_buffer: String,

    /// Track ordered list counters (Some(n) = ordered starting at n).
    pub(super) list_indices: Vec<Option<u64>>,

    /// Nesting depth for list indentation.
    pub(super) list_depth: usize,

    /// Blockquote nesting depth.
    pub(super) blockquote_depth: usize,

    /// Identity of the link whose label is currently being collected.
    pub(super) link: Option<usize>,

    /// Whether we need a blank line before the next block element.
    pub(super) needs_newline: bool,

    /// Whether the next item line needs a list marker.
    pub(super) pending_marker: bool,

    /// Whether we have emitted any content yet.
    pub(super) has_output: bool,

    /// Inside a table.
    pub(super) in_table: bool,

    /// Collecting table rows: each row is a vec of cell-span groups.
    pub(super) table_rows: Vec<Vec<Vec<InlineSpan>>>,

    /// Current row's cells being accumulated.
    pub(super) table_current_row: Vec<Vec<InlineSpan>>,

    /// Spans for the current table cell.
    pub(super) table_cell_spans: Vec<InlineSpan>,
}

impl<'a, I: Iterator<Item = Event<'a>>> Writer<'a, I> {
    pub(super) fn new(
        iter: I,
        default_code_lang: Option<String>,
        wrap_width: Option<usize>,
        link_display: &'a dyn Fn(&str) -> LinkDisplay,
    ) -> Self {
        Self {
            iter,
            lines: Vec::new(),
            text: Vec::new(),
            links: Vec::new(),
            link_display,
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

    pub(super) fn run(&mut self) {
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
                self.link = Some(self.links.len());
                self.links.push(RenderedLink {
                    destination: dest_url.to_string(),
                    label: String::new(),
                    text_ranges: Vec::new(),
                    fragments: Vec::new(),
                });
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
                if let Some(index) = self.link.take() {
                    let link = &self.links[index];
                    if (self.link_display)(&link.destination) == LinkDisplay::LabelAndDestination
                        && link.label != link.destination
                    {
                        // Added destination text is readable fallback, not part of the label.
                        self.push_span(Span::styled(
                            format!(" ({})", link.destination),
                            self.styles.link,
                        ));
                    }
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
    // -- helpers ------------------------------------------------------------

    pub(super) fn current_style(&self) -> Style {
        let mut s = if self.link.is_some() {
            self.styles.link
        } else {
            Style::default()
        };
        for sty in &self.inline_styles {
            s = s.patch(*sty);
        }
        s
    }

    pub(super) fn push_span(&mut self, span: Span<'static>) {
        if let Some(index) = self.link {
            self.links[index].label.push_str(&span.content);
        }
        let span = InlineSpan {
            span,
            link: self.link,
        };
        if self.in_table {
            self.table_cell_spans.push(span);
        } else {
            self.current_spans.push(span);
        }
    }

    pub(super) fn push_prefix(&mut self, span: Span<'static>) {
        self.current_spans.push(InlineSpan { span, link: None });
    }

    /// Prepend blockquote / list-marker prefix to the current line, if needed.
    pub(super) fn emit_prefix_if_needed(&mut self) {
        // Only emit prefix at the start of a new line (empty current_spans)
        if !self.current_spans.is_empty() {
            return;
        }

        // Blockquote prefix
        for _ in 0..self.blockquote_depth {
            self.push_prefix(Span::styled("> ", self.styles.blockquote_prefix));
        }

        // List marker / indentation
        if self.list_depth > 0 {
            // Indentation for nesting (each level except the innermost)
            let indent_levels = self.list_depth.saturating_sub(1);
            if indent_levels > 0 {
                self.push_prefix(Span::raw("  ".repeat(indent_levels)));
            }

            if self.pending_marker {
                // Emit marker
                if let Some(maybe_idx) = self.list_indices.last() {
                    match maybe_idx {
                        Some(idx) => {
                            self.push_prefix(Span::raw(format!("{}. ", idx)));
                        }
                        None => {
                            self.push_prefix(Span::raw("- "));
                        }
                    }
                }
                self.pending_marker = false;
            } else {
                // Continuation indent (align after marker)
                self.push_prefix(Span::raw("  "));
            }
        }
    }

    pub(super) fn flush_line(&mut self) {
        if self.current_spans.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.current_spans);
        let text_index = self.push_search_text(&spans);
        let spans = spans.into_iter().map(|span| span.span).collect::<Vec<_>>();

        // Apply word-wrapping for prose content (not code blocks, tables, or headings).
        if let Some(width) = self.wrap_width
            && !self.in_code_block
            && !self.in_table
            && self.in_heading.is_none()
        {
            let indent = self.continuation_indent();
            for wrapped in wrap_spans_with_ranges(&spans, width, indent) {
                self.text[text_index].fragments.push(TextFragment {
                    line: self.lines.len(),
                    source_range: wrapped.source_range,
                    visual_start: wrapped.indent,
                });
                self.lines.push(Line::from(wrapped.spans));
            }
            self.has_output = true;
            return;
        }

        self.push_line_with_text(Line::from(spans), text_index);
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

    pub(super) fn push_blank_line(&mut self) {
        self.lines.push(Line::from(""));
    }

    pub(super) fn push_search_text(&mut self, spans: &[InlineSpan]) -> usize {
        let index = self.text.len();
        let mut text = String::new();
        for span in spans {
            let start = text.len();
            text.push_str(&span.span.content);
            if let Some(link) = span.link {
                let ranges = &mut self.links[link].text_ranges;
                if let Some(last) = ranges.last_mut()
                    && last.text_index == index
                    && last.range.end == start
                {
                    last.range.end = text.len();
                } else {
                    ranges.push(LinkTextRange {
                        text_index: index,
                        range: start..text.len(),
                    });
                }
            }
        }
        self.text.push(RenderedText {
            text,
            fragments: Vec::new(),
        });
        index
    }

    pub(super) fn push_unwrapped_line(&mut self, line: Line<'static>) {
        let spans = line
            .spans
            .iter()
            .cloned()
            .map(|span| InlineSpan { span, link: None })
            .collect::<Vec<_>>();
        let index = self.push_search_text(&spans);
        self.push_line_with_text(line, index);
    }

    pub(super) fn push_line_with_text(&mut self, line: Line<'static>, index: usize) {
        let len = self.text[index].text.len();
        self.text[index].fragments.push(TextFragment {
            line: self.lines.len(),
            source_range: 0..len,
            visual_start: 0,
        });
        self.lines.push(line);
    }

    fn ensure_blank_line_before_block(&mut self) {
        if self.needs_newline && self.has_output {
            self.flush_line();
            self.push_blank_line();
            self.needs_newline = false;
        }
    }
}
