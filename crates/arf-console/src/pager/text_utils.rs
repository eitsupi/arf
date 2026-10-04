//! Display-width-aware text utilities for terminal rendering.
//!
//! All width calculations use display columns (not character count), so
//! full-width characters (CJK, some emoji) correctly occupy 2 columns.

use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

/// Return the display width of a string in terminal columns.
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Truncate a string so it fits within `max_width` display columns.
///
/// If the string is longer than `max_width`, the last visible column
/// is replaced with `…`. When a wide character would straddle the
/// boundary, it is dropped and the gap is *not* filled with a space
/// (the trailing `…` occupies that column instead).
pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    if display_width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return "…".to_string();
    }

    let target = max_width - 1; // reserve 1 col for '…'
    let mut col = 0;
    let mut end = 0;
    for (i, ch) in s.char_indices() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + w > target {
            break;
        }
        col += w;
        end = i + ch.len_utf8();
    }

    let mut out = s[..end].to_string();
    out.push('…');
    out
}

/// Return `true` if the string's display width exceeds `max_width`.
pub fn exceeds_width(s: &str, max_width: usize) -> bool {
    display_width(s) > max_width
}

/// Produce a scrolling window of text for animation.
///
/// `scroll_pos` is measured in **display columns** (not characters).
///
/// Returns `(visible_string, max_scroll)`.
///
/// * At `scroll_pos == 0`: beginning shown, `…` at end.
/// * At `scroll_pos >= max_scroll`: end shown, `…` at start.
/// * In between: `…` on both sides.
pub fn scroll_display(s: &str, max_width: usize, scroll_pos: usize) -> (String, usize) {
    if max_width == 0 {
        return (String::new(), 0);
    }

    let total = display_width(s);

    if total <= max_width {
        return (s.to_string(), 0);
    }

    // With only 1 column there is no room to show content; always show '…'.
    if max_width == 1 {
        return ("…".to_string(), total);
    }

    // max_scroll = how many columns we can shift before reaching the end.
    // At position 0 we show (max_width - 1) content cols + trailing '…'.
    // At max_scroll we show leading '…' + (max_width - 1) content cols.
    let max_scroll = total.saturating_sub(max_width.saturating_sub(1));
    let eff = scroll_pos.min(max_scroll);

    if eff == 0 {
        // Beginning: show first (max_width-1) cols + '…'
        let (text, actual_vis) = take_columns(s, max_width - 1);
        let right_pad = (max_width - 1).saturating_sub(actual_vis);
        (format!("{}{}…", text, " ".repeat(right_pad)), max_scroll)
    } else if eff >= max_scroll {
        // End: '…' + last (max_width-1) cols
        let skip_cols = total.saturating_sub(max_width - 1);
        let (remainder, actual_skipped) = skip_columns(s, skip_cols);
        // If a wide char straddled the boundary, pad to maintain exact width
        let overshoot = actual_skipped.saturating_sub(skip_cols);
        (
            format!("…{}{}", " ".repeat(overshoot), remainder),
            max_scroll,
        )
    } else {
        // Middle: '…' + (max_width-2) cols + '…'
        let inner_cols = max_width.saturating_sub(2);
        let (after_skip, actual_skipped) = skip_columns(s, eff);
        // Compensate for wide-char overshoot: pad left, reduce content.
        // Clamp overshoot to inner_cols so we never exceed max_width.
        let overshoot = actual_skipped.saturating_sub(eff).min(inner_cols);
        let content_cols = inner_cols.saturating_sub(overshoot);
        let (visible, actual_vis) = take_columns(&after_skip, content_cols);
        let right_pad = inner_cols.saturating_sub(overshoot + actual_vis);
        (
            format!(
                "…{}{}{}…",
                " ".repeat(overshoot),
                visible,
                " ".repeat(right_pad)
            ),
            max_scroll,
        )
    }
}

/// Pad (or truncate) a string to exactly `width` display columns.
///
/// When the string is wider than `width`, it is silently truncated
/// **without** an ellipsis.  Callers that want ellipsis on overflow
/// should call [`truncate_to_width`] first.
pub fn pad_to_width(s: &str, width: usize) -> String {
    let w = display_width(s);
    if w >= width {
        // Need to truncate (no ellipsis — this is padding, not user-facing truncation)
        let (text, actual) = take_columns(s, width);
        if actual < width {
            format!("{}{}", text, " ".repeat(width - actual))
        } else {
            text
        }
    } else {
        format!("{}{}", s, " ".repeat(width - w))
    }
}

/// Wrap a sequence of styled spans to fit within `max_width` display columns.
///
/// Returns a `Vec` of lines, where each line is a `Vec<Span>`.  If the total
/// width of the input fits within `max_width`, returns it as a single-element
/// vec unchanged.
///
/// - Breaks at word boundaries (spaces) and after CJK characters.
/// - Falls back to character-level wrapping for words longer than the
///   available width.
/// - Continuation lines are indented by `continuation_indent` spaces.
/// - Span styles are preserved across split boundaries.
#[cfg(test)]
fn wrap_spans(
    spans: &[Span<'static>],
    max_width: usize,
    continuation_indent: usize,
) -> Vec<Vec<Span<'static>>> {
    wrap_spans_with_ranges(spans, max_width, continuation_indent)
        .into_iter()
        .map(|line| line.spans)
        .collect()
}

/// A wrapped line's text and its UTF-8 byte range in the original spans.
pub(super) struct WrappedLine {
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

/// Wrap spans while retaining the source ranges, excluding trimmed break spaces.
pub(super) fn wrap_spans_with_ranges(
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

// -- helpers ----------------------------------------------------------------

/// Take up to `cols` display columns from the front of `s`.
///
/// Returns `(substring, actual_columns_taken)`.  If a wide character
/// would straddle the boundary it is not included, so `actual` may be
/// less than `cols`.
fn take_columns(s: &str, cols: usize) -> (String, usize) {
    let mut col = 0;
    let mut end = 0;
    for (i, ch) in s.char_indices() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + w > cols {
            break;
        }
        col += w;
        end = i + ch.len_utf8();
    }
    (s[..end].to_string(), col)
}

/// Skip `cols` display columns from the front and return the remainder
/// along with the actual number of columns skipped.
///
/// If a wide character straddles the boundary, it is included in the skip
/// (we advance to the next character boundary at or beyond `cols`), so
/// `actual_skipped` may be greater than `cols` by up to 1.
fn skip_columns(s: &str, cols: usize) -> (String, usize) {
    let mut col = 0;
    for (i, ch) in s.char_indices() {
        if col >= cols {
            return (s[i..].to_string(), col);
        }
        col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    (String::new(), col)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── display_width ──────────────────────────────────────────────────

    #[test]
    fn ascii_width() {
        assert_eq!(display_width("hello"), 5);
    }

    #[test]
    fn cjk_width() {
        // Each CJK ideograph is 2 columns.
        assert_eq!(display_width("日本語"), 6);
    }

    #[test]
    fn mixed_width() {
        assert_eq!(display_width("hi日本"), 6); // 2 + 2*2
    }

    #[test]
    fn empty_width() {
        assert_eq!(display_width(""), 0);
    }

    // ── truncate_to_width ──────────────────────────────────────────────

    #[test]
    fn truncate_ascii_no_op() {
        assert_eq!(truncate_to_width("hello", 10), "hello");
        assert_eq!(truncate_to_width("hello", 5), "hello");
    }

    #[test]
    fn truncate_ascii() {
        assert_eq!(truncate_to_width("hello world", 8), "hello w…");
    }

    #[test]
    fn truncate_cjk() {
        // "日本語テスト" = 12 cols.  max_width = 7 → 6 content cols + '…'
        // 日(2)+本(2)+語(2) = 6, fits 6 content cols.
        assert_eq!(truncate_to_width("日本語テスト", 7), "日本語…");
    }

    #[test]
    fn truncate_cjk_boundary() {
        // max_width = 6 → 5 content cols. 日(2)+本(2)=4, 語 would need 6 → skip.
        assert_eq!(truncate_to_width("日本語テスト", 6), "日本…");
    }

    #[test]
    fn truncate_edge_min() {
        assert_eq!(truncate_to_width("hello", 1), "…");
    }

    #[test]
    fn truncate_zero_width() {
        assert_eq!(truncate_to_width("hello", 0), "");
    }

    #[test]
    fn truncate_empty() {
        assert_eq!(truncate_to_width("", 5), "");
    }

    // ── exceeds_width ──────────────────────────────────────────────────

    #[test]
    fn exceeds_ascii() {
        assert!(!exceeds_width("hello", 10));
        assert!(exceeds_width("hello world", 8));
    }

    #[test]
    fn exceeds_cjk() {
        assert!(exceeds_width("日本語", 5)); // 6 cols > 5
        assert!(!exceeds_width("日本語", 6));
    }

    // ── scroll_display ─────────────────────────────────────────────────

    #[test]
    fn scroll_fits() {
        let (r, m) = scroll_display("hello", 10, 0);
        assert_eq!(r, "hello");
        assert_eq!(m, 0);
    }

    #[test]
    fn scroll_start_ascii() {
        let (r, m) = scroll_display("hello world", 8, 0);
        assert_eq!(r, "hello w…");
        assert_eq!(m, 4); // 11 - 7 = 4
    }

    #[test]
    fn scroll_end_ascii() {
        let (r, _) = scroll_display("hello world", 8, 100);
        assert_eq!(r, "…o world");
    }

    #[test]
    fn scroll_middle_ascii() {
        let (r, _) = scroll_display("hello world", 8, 2);
        assert_eq!(r, "…llo wo…");
    }

    #[test]
    fn scroll_cjk_start() {
        // "日本語テスト" = 12 cols, max=7 → show 6 cols + '…'
        let (r, m) = scroll_display("日本語テスト", 7, 0);
        assert_eq!(r, "日本語…");
        assert_eq!(m, 6); // 12 - 6 = 6
    }

    #[test]
    fn scroll_cjk_end() {
        // 12 cols total, max_width=7 → show '…' + last 6 cols = "テスト"
        let (r, _) = scroll_display("日本語テスト", 7, 100);
        assert_eq!(r, "…テスト");
    }

    // ── pad_to_width ───────────────────────────────────────────────────

    #[test]
    fn pad_ascii() {
        assert_eq!(pad_to_width("hi", 5), "hi   ");
    }

    #[test]
    fn pad_exact() {
        assert_eq!(pad_to_width("hello", 5), "hello");
    }

    #[test]
    fn pad_cjk() {
        // "日本" = 4 cols, pad to 6 → 2 spaces
        assert_eq!(pad_to_width("日本", 6), "日本  ");
    }

    #[test]
    fn pad_truncate() {
        // "hello world" wider than 5 → take 5 cols
        assert_eq!(pad_to_width("hello world", 5), "hello");
    }

    #[test]
    fn pad_truncate_cjk_boundary() {
        // "日本語" = 6 cols, pad to 5 → 日(2)+本(2)=4, 語 doesn't fit → "日本 "
        assert_eq!(pad_to_width("日本語", 5), "日本 ");
    }

    // ── helpers ─────────────────────────────────────────────────────────

    #[test]
    fn take_columns_basic() {
        let (s, c) = take_columns("hello", 3);
        assert_eq!(s, "hel");
        assert_eq!(c, 3);
    }

    #[test]
    fn take_columns_cjk_boundary() {
        // "日本語" = 6 cols. take 3 → 日(2) fits, 本 needs 4 → stop
        let (s, c) = take_columns("日本語", 3);
        assert_eq!(s, "日");
        assert_eq!(c, 2);
    }

    #[test]
    fn skip_columns_basic() {
        let (s, actual) = skip_columns("hello world", 6);
        assert_eq!(s, "world");
        assert_eq!(actual, 6);
    }

    #[test]
    fn skip_columns_cjk() {
        // skip 4 cols from "日本語テスト" → skip 日(2)+本(2), remainder = "語テスト"
        let (s, actual) = skip_columns("日本語テスト", 4);
        assert_eq!(s, "語テスト");
        assert_eq!(actual, 4);
    }

    #[test]
    fn skip_columns_cjk_odd_boundary() {
        // skip 3 cols from all-wide text: 日(2) → col=2 < 3, 本(2) → col=4 >= 3
        // Overshoot: asked for 3, actually skipped 4
        let (s, actual) = skip_columns("日本語テスト", 3);
        assert_eq!(s, "語テスト");
        assert_eq!(actual, 4);
    }

    // ── scroll_display overshoot compensation ──────────────────────────

    #[test]
    fn scroll_cjk_middle_odd_pos() {
        // "日本語テスト" = 12 cols, max_width = 8
        // max_scroll = 12 - 7 = 5
        // eff = 3 (odd): skip_columns skips 4 (overshoot=1)
        // inner_cols = 8-2 = 6, content_cols = 6-1 = 5
        // after_skip = "語テスト" (8 cols), take 5 → "語テ" (4 cols) + right_pad=1
        // Result: "… 語テ …" with exact width = 1+1+4+1+1 = 8
        let (r, _) = scroll_display("日本語テスト", 8, 3);
        assert_eq!(r, "…\u{0020}語テ\u{0020}…");
        assert_eq!(display_width(&r), 8);
    }

    #[test]
    fn scroll_cjk_end_odd_boundary() {
        // "日本語テスト" = 12 cols, max_width = 8
        // End: skip_cols = 12 - 7 = 5 (odd for all-wide text → overshoot)
        // Result must still be exactly 8 cols wide
        let (r, _) = scroll_display("日本語テスト", 8, 100);
        assert_eq!(display_width(&r), 8);
        assert!(r.starts_with('…'));
    }

    // ── emoji ──────────────────────────────────────────────────────────

    #[test]
    fn emoji_display_width() {
        // Pin the expected width for unicode-width 0.2: single-codepoint emoji = 2 cols.
        // If this fails after a unicode-width upgrade, update the expectation.
        assert_eq!(display_width("🎉"), 2);
    }

    #[test]
    fn truncate_emoji() {
        // "🎉🎊🎁" = 6 cols (each emoji 2 cols), max_width = 3
        // target = 2 content cols → 🎉(2) fits, 🎊 would need 4 → stop → "🎉…"
        assert_eq!(truncate_to_width("🎉🎊🎁", 3), "🎉…");
    }

    // ── truncate for "Copied" message (meta_command.rs) ──────────────

    #[test]
    fn truncate_copied_message_cjk() {
        // Simulate the "Copied: ..." display truncation at 60 columns.
        // A CJK-heavy command that exceeds 60 display columns must be
        // truncated with '…' and fit within the budget.
        let cmd = "変数名 <- read.csv('非常に長いファイルパス/データセット.csv')";
        let result = truncate_to_width(cmd, 60);
        assert!(display_width(&result) <= 60);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_copied_message_short_no_op() {
        // A short command should pass through unchanged.
        let cmd = "print('hello')";
        assert_eq!(truncate_to_width(cmd, 60), cmd);
    }

    // ── edge cases from PR #39 review ────────────────────────────────

    #[test]
    fn scroll_zero_width() {
        // max_width==0 must not panic and should return empty string
        let (r, m) = scroll_display("hello world", 0, 0);
        assert_eq!(r, "");
        assert_eq!(m, 0);
        // Non-zero scroll_pos should also be caught by the early guard
        let (r2, m2) = scroll_display("hello world", 0, 5);
        assert_eq!(r2, "");
        assert_eq!(m2, 0);
    }

    #[test]
    fn scroll_width_one() {
        // max_width==1: only room for '…' at every scroll position
        let (r, m) = scroll_display("hello world", 1, 0);
        assert_eq!(r, "…");
        assert_eq!(display_width(&r), 1);
        assert_eq!(m, 11); // total = 11

        let (r2, _) = scroll_display("hello world", 1, 5);
        assert_eq!(r2, "…");
        assert_eq!(display_width(&r2), 1);
    }

    #[test]
    fn scroll_width_two_cjk() {
        // max_width==2 with CJK: middle branch must not exceed 2 cols
        // "日本語" = 6 cols, max_width=2
        // Start: take_columns(s, 1) → ("", 0), pad 1 → " …" = 2 cols
        let (r, _) = scroll_display("日本語", 2, 0);
        assert_eq!(r, " …");
        assert_eq!(display_width(&r), 2);
        // Middle: inner_cols=0, overshoot clamped to 0 → "……" = 2 cols
        let (r2, _) = scroll_display("日本語", 2, 1);
        assert_eq!(r2, "……");
        assert_eq!(display_width(&r2), 2);
        // End: "…" + last 1 col → CJK can't fit in 1 col → "… " = 2 cols
        let (r3, _) = scroll_display("日本語", 2, 100);
        assert_eq!(r3, "… ");
        assert_eq!(display_width(&r3), 2);
    }

    #[test]
    fn scroll_start_cjk_boundary_padding() {
        // "日本語テスト" = 12 cols, max_width = 5
        // eff==0 branch: take_columns(s, 4) → 日(2)+本(2)=4, actual_vis=4
        // Result: "日本…" = 5 cols — no padding needed here
        let (r, _) = scroll_display("日本語テスト", 5, 0);
        assert_eq!(r, "日本…");
        // max_width = 4: take_columns(s, 3) → 日(2), actual_vis=2, pad 1
        // Result: "日 …" = 4 cols
        let (r2, _) = scroll_display("日本語テスト", 4, 0);
        assert_eq!(display_width(&r2), 4);
        assert_eq!(r2, "日 …");
    }

    // ── wrap_spans ────────────────────────────────────────────────────

    /// Helper to collect wrapped lines as plain text strings.
    fn wrap_plain(text: &str, max_width: usize, indent: usize) -> Vec<String> {
        let spans = vec![Span::raw(text.to_string())];
        wrap_spans(&spans, max_width, indent)
            .into_iter()
            .map(|line| line.iter().map(|s| s.content.as_ref()).collect())
            .collect()
    }

    #[test]
    fn wrap_no_op_when_fits() {
        assert_eq!(wrap_plain("hello world", 20, 0), vec!["hello world"]);
    }

    #[test]
    fn wrap_at_word_boundary() {
        assert_eq!(
            wrap_plain("hello world foo", 12, 0),
            vec!["hello world", "foo"]
        );
    }

    #[test]
    fn wrap_continuation_indent() {
        assert_eq!(
            wrap_plain("hello world foo", 12, 2),
            vec!["hello world", "  foo"]
        );
    }

    #[test]
    fn wrap_long_word_char_break() {
        // "abcdefghij" (10 chars) with max_width 5 → char-level wrap
        assert_eq!(wrap_plain("abcdefghij", 5, 0), vec!["abcde", "fghij"]);
    }

    #[test]
    fn wrap_cjk_break() {
        // "日本語テスト" = 12 cols, max_width 8 → break after a CJK char
        let result = wrap_plain("日本語テスト", 8, 0);
        assert_eq!(result.len(), 2);
        // First line: 日(2)+本(2)+語(2)+テ(2)=8 fits, but テ would make it go over...
        // Actually: 日(2)=2, 本(2)=4, 語(2)=6, テ(2)=8 → all fit in 8
        // But then スト doesn't fit, break after テ
        assert_eq!(result[0], "日本語テ");
        assert_eq!(result[1], "スト");
    }

    #[test]
    fn wrap_preserves_styles() {
        use ratatui::style::Style;
        let bold = Style::new().bold();
        let spans = vec![
            Span::raw("hello ".to_string()),
            Span::styled("world foo".to_string(), bold),
        ];
        let lines = wrap_spans(&spans, 10, 0);
        assert_eq!(lines.len(), 2);
        // First line: "hello " + "worl" (styled) → "hello worl"? No...
        // "hello " (6) + "world" is 11 total > 10
        // "hello " (6) + "wor" (9) + "l" (10) + "d" (11) → break after space in "world foo"
        // Actually "hello world foo" = 15 cols. At 10 cols with word boundary:
        // "hello" (5) + " " (6) + "world" (11) → overflows at 11
        // break point after "hello " → first line "hello", second line "world foo"
        // Wait, let's trace: pos=0, col=0, line_width=10
        // 'h'(1) col=1, 'e'(1) col=2, 'l'(1) col=3, 'l'(1) col=4, 'o'(1) col=5, ' '(1) col=6 last_break=6
        // 'w'(1) col=7, 'o'(1) col=8, 'r'(1) col=9, 'l'(1) col=10
        // 'd'(1) col=11 > 10 → break. end=10. last_break=6. split=6.
        // trim trailing spaces: chars[5].is_space → trim_end=5
        // line 1: chars[0..5] = "hello"
        // pos=6, skip leading spaces: chars[6]='w', no skip
        // Actually wait, pos=split=6, chars[6]='w' not space → no skip
        // line 2: "world foo" → fits in 10-indent? yes with indent=0
        let line1_text: String = lines[0].iter().map(|s| s.content.as_ref()).collect();
        let line2_text: String = lines[1].iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(line1_text, "hello");
        assert_eq!(line2_text, "world foo");
        // Check that "world foo" retains bold style
        let styled_span = lines[1]
            .iter()
            .find(|s| s.content.as_ref().contains("world"));
        assert!(styled_span.is_some());
        assert!(
            styled_span
                .unwrap()
                .style
                .add_modifier
                .contains(ratatui::style::Modifier::BOLD)
        );
    }

    #[test]
    fn wrap_zero_width_no_panic() {
        let result = wrap_plain("hello", 0, 0);
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn wrap_empty_input() {
        let spans: Vec<Span<'static>> = vec![];
        let result = wrap_spans(&spans, 10, 0);
        assert_eq!(result.len(), 1);
        assert!(result[0].is_empty());
    }

    #[test]
    fn wrap_multiple_spaces_collapsed() {
        // After wrapping at a space, leading spaces on next line are skipped
        let result = wrap_plain("aaa   bbb", 4, 0);
        assert_eq!(result, vec!["aaa", "bbb"]);
    }

    #[test]
    fn wrap_spans_line_width_zero_fallback() {
        // When continuation_indent >= max_width, line_width becomes 0 on
        // continuation lines.  The fallback should emit one char at a time
        // rather than dropping text.
        let spans = vec![Span::raw("abc")];
        let result = wrap_spans(&spans, 2, 3); // continuation indent (3) > max_width (2)
        // First line fits "ab" (max_width=2), then continuation lines have
        // line_width = 2 - 3 = 0, so fallback emits one char at a time.
        assert_eq!(result.len(), 2); // "ab" + "c"
        let text: String = result
            .iter()
            .map(|line| line.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect::<Vec<_>>()
            .join("|");
        assert_eq!(text, "ab|c");
    }
}
