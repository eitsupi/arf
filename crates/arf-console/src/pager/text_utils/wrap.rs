//! Span-level text wrapping that tracks source byte ranges.

use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

/// A wrapped line's text and its UTF-8 byte range in the original spans.
pub(in crate::pager) struct WrappedLine {
    pub spans: Vec<Span<'static>>,
    pub source_range: std::ops::Range<usize>,
    /// Bytes inserted before source text for continuation indentation.
    pub indent: usize,
}

impl WrappedLine {
    pub fn unwrapped(spans: Vec<Span<'static>>) -> Self {
        let len = spans.iter().map(|span| span.content.len()).sum();
        Self {
            spans,
            source_range: 0..len,
            indent: 0,
        }
    }
}

/// Wrap a sequence of styled spans to fit within `max_width` display columns.
///
/// Returns the wrapped lines together with their source ranges, excluding
/// trimmed break spaces. If the total width of the input fits within
/// `max_width`, returns it as a single unwrapped line.
///
/// - Breaks at word boundaries (spaces) and after CJK characters.
/// - Falls back to character-level wrapping for words longer than the
///   available width.
/// - Continuation lines are indented by `continuation_indent` spaces.
/// - Span styles are preserved across split boundaries.
pub(in crate::pager) fn wrap_spans_with_ranges(
    spans: &[Span<'static>],
    max_width: usize,
    continuation_indent: usize,
) -> Vec<WrappedLine> {
    use ratatui::style::Style;
    use unicode_width::UnicodeWidthChar;

    if max_width == 0 {
        return vec![WrappedLine::unwrapped(spans.to_vec())];
    }

    // Fast path: check if wrapping is needed at all.
    let total_width: usize = spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    if total_width <= max_width {
        return vec![WrappedLine::unwrapped(spans.to_vec())];
    }

    // Per-character metadata for wrapping decisions.
    struct CI {
        ch: char,
        width: usize,
        style: Style,
        is_space: bool,
        is_cjk: bool,
        byte_offset: usize,
    }

    /// Build a `Vec<Span>` from a slice of CI, coalescing adjacent chars
    /// with the same style.
    fn build_line(chars: &[CI], is_continuation: bool, indent: usize) -> WrappedLine {
        let source_start = chars.first().map_or(0, |ch| ch.byte_offset);
        let source_end = chars
            .last()
            .map_or(source_start, |ch| ch.byte_offset + ch.ch.len_utf8());
        let indent = if is_continuation { indent } else { 0 };
        let mut spans: Vec<Span<'static>> = Vec::new();
        if indent > 0 {
            spans.push(Span::raw(" ".repeat(indent)));
        }
        if chars.is_empty() {
            return WrappedLine {
                spans,
                source_range: source_start..source_end,
                indent,
            };
        }
        let mut cur_style = chars[0].style;
        let mut cur_text = String::new();
        for ci in chars {
            if ci.style == cur_style {
                cur_text.push(ci.ch);
            } else {
                if !cur_text.is_empty() {
                    spans.push(Span::styled(cur_text, cur_style));
                    cur_text = String::new();
                }
                cur_style = ci.style;
                cur_text.push(ci.ch);
            }
        }
        if !cur_text.is_empty() {
            spans.push(Span::styled(cur_text, cur_style));
        }
        WrappedLine {
            spans,
            source_range: source_start..source_end,
            indent,
        }
    }

    let mut chars: Vec<CI> = Vec::new();
    let mut byte_offset = 0;
    for span in spans {
        let style = span.style;
        for ch in span.content.chars() {
            let w = UnicodeWidthChar::width(ch).unwrap_or(0);
            chars.push(CI {
                ch,
                width: w,
                style,
                is_space: ch == ' ',
                is_cjk: w == 2,
                byte_offset,
            });
            byte_offset += ch.len_utf8();
        }
    }

    let mut result: Vec<WrappedLine> = Vec::new();
    let mut pos = 0;

    while pos < chars.len() {
        let is_continuation = !result.is_empty();
        let line_width = if is_continuation {
            max_width.saturating_sub(continuation_indent)
        } else {
            max_width
        };

        if line_width == 0 {
            // Continuation indent consumed all available width.
            // Fall back to wrapping without indent to avoid dropping text.
            result.push(build_line(&chars[pos..pos + 1], false, 0));
            pos += 1;
            continue;
        }

        let mut col = 0;
        let mut end = pos;
        let mut last_break = None;

        while end < chars.len() {
            let ci = &chars[end];
            if col + ci.width > line_width {
                break;
            }
            col += ci.width;
            end += 1;

            if ci.is_space || (ci.is_cjk && end < chars.len()) {
                last_break = Some(end);
            }
        }

        if end == chars.len() {
            result.push(build_line(
                &chars[pos..end],
                is_continuation,
                continuation_indent,
            ));
            break;
        }

        let split = if let Some(bp) = last_break {
            if bp > pos { bp } else { end.max(pos + 1) }
        } else {
            end.max(pos + 1)
        };

        // Trim trailing spaces at the break.
        let mut trim_end = split;
        while trim_end > pos && chars[trim_end - 1].is_space {
            trim_end -= 1;
        }

        result.push(build_line(
            &chars[pos..trim_end],
            is_continuation,
            continuation_indent,
        ));

        pos = split;
        while pos < chars.len() && chars[pos].is_space {
            pos += 1;
        }
    }

    if result.is_empty() {
        vec![WrappedLine::unwrapped(spans.to_vec())]
    } else {
        result
    }
}
