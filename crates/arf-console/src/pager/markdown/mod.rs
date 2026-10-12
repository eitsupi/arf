//! Markdown to ratatui `Line` renderer.
//!
//! Converts CommonMark text into styled `Vec<Line<'static>>` suitable for
//! the pager, optionally wrapping prose and table cells. Search text retains
//! its logical boundaries and a mapping to the wrapped lines.
//!
//! Reference: `refs/codex/codex-rs/tui2/src/markdown_render.rs`

mod code_block;
mod inline;
mod style;
mod table;
mod writer;

use pulldown_cmark::{Options, Parser};
use ratatui::text::{Line, Span};
use std::ops::Range;
use unicode_width::UnicodeWidthStr;
use writer::Writer;

/// Render a Markdown string into styled ratatui lines.
///
/// `default_code_lang` is used for fenced code blocks that have no language tag.
/// Pass `Some("r")` when rendering R documentation (help pages, vignettes) so
/// that untagged code blocks receive R syntax highlighting.
///
/// `wrap_width` enables word-wrapping for prose content (paragraphs,
/// blockquotes, list items) and table cells. Code blocks and headings are never
/// wrapped. Pass `None` to disable wrapping (every logical line maps to
/// exactly one `Line`).
pub fn render_markdown(
    input: &str,
    default_code_lang: Option<&str>,
    wrap_width: Option<usize>,
) -> Vec<Line<'static>> {
    render_markdown_with_text(input, default_code_lang, wrap_width).lines
}

/// Render Markdown with width-independent text and its visual positions.
pub(super) fn render_markdown_with_text(
    input: &str,
    default_code_lang: Option<&str>,
    wrap_width: Option<usize>,
) -> RenderedMarkdown {
    render_markdown_document(input, default_code_lang, wrap_width, |_| {
        LinkDisplay::LabelAndDestination
    })
}

/// Control destination text independently of semantic link metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LinkDisplay {
    LabelOnly,
    LabelAndDestination,
}

/// Render text and link positions with a consumer-supplied display policy.
///
/// Destinations remain opaque to the renderer. A link's index in `links` is its
/// identity, including when several links share a destination. Re-rendering the
/// same source preserves these indices; visual positions reflect the new width.
/// Matching label/destination text is shown once, including for autolinks.
pub(super) fn render_markdown_document(
    input: &str,
    default_code_lang: Option<&str>,
    wrap_width: Option<usize>,
    link_display: impl Fn(&str) -> LinkDisplay,
) -> RenderedMarkdown {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let parser = Parser::new_ext(input, options);
    let mut writer = Writer::new(
        parser,
        default_code_lang.map(|s| s.to_string()),
        wrap_width,
        &link_display,
    );
    writer.run();
    let mut document = RenderedMarkdown {
        lines: writer.lines,
        text: writer.text,
        links: writer.links,
    };
    document.project_links();
    document
}

pub(super) struct RenderedMarkdown {
    pub lines: Vec<Line<'static>>,
    pub text: Vec<RenderedText>,
    pub links: Vec<RenderedLink>,
}

pub(super) struct RenderedLink {
    pub destination: String,
    pub label: String,
    pub text_ranges: Vec<LinkTextRange>,
    pub fragments: Vec<LinkFragment>,
}

pub(super) struct LinkTextRange {
    /// Index into the document's unwrapped rendered text.
    pub text_index: usize,
    /// UTF-8 bytes in that logical text; excludes added destinations/prefixes.
    pub range: Range<usize>,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct LinkFragment {
    pub line: usize,
    /// UTF-8 bytes in the concatenated visual-line spans.
    pub byte_range: Range<usize>,
    /// Terminal display columns, independent of UTF-8 byte offsets.
    pub columns: Range<usize>,
}

impl RenderedMarkdown {
    fn project_links(&mut self) {
        for link in &mut self.links {
            for label in &link.text_ranges {
                for fragment in &self.text[label.text_index].fragments {
                    let start = label.range.start.max(fragment.source_range.start);
                    let end = label.range.end.min(fragment.source_range.end);
                    if start >= end {
                        continue;
                    }
                    let start = fragment.visual_start + start - fragment.source_range.start;
                    let end = fragment.visual_start + end - fragment.source_range.start;
                    let line = &self.lines[fragment.line];
                    link.fragments.push(LinkFragment {
                        line: fragment.line,
                        byte_range: start..end,
                        columns: display_column(line, start)..display_column(line, end),
                    });
                }
            }
        }
    }
}

/// Count columns span by span, as ratatui does when laying out styled text.
fn display_column(line: &Line<'_>, mut byte_offset: usize) -> usize {
    let mut columns = 0;
    for span in &line.spans {
        let bytes = byte_offset.min(span.content.len());
        columns += UnicodeWidthStr::width(&span.content[..bytes]);
        byte_offset -= bytes;
        if byte_offset == 0 {
            break;
        }
    }
    columns
}

/// One logical line, or one table cell line, before terminal wrapping.
pub(super) struct RenderedText {
    pub text: String,
    pub fragments: Vec<TextFragment>,
}

pub(super) struct TextFragment {
    pub line: usize,
    pub source_range: Range<usize>,
    /// UTF-8 byte offset in the concatenated spans of the visual line.
    pub visual_start: usize,
}

/// Inline text carries a link identity until its logical text is recorded.
#[derive(Clone)]
struct InlineSpan {
    span: Span<'static>,
    link: Option<usize>,
}

#[cfg(test)]
mod tests;
