//! Display-width-aware text utilities for terminal rendering.
//!
//! All width calculations use display columns (not character count), so
//! full-width characters (CJK, some emoji) correctly occupy 2 columns.

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
        // Compensate for wide-char overshoot: pad left, reduce content
        let overshoot = actual_skipped.saturating_sub(eff);
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

    // ── edge cases from PR #39 review ────────────────────────────────

    #[test]
    fn scroll_zero_width() {
        // max_width==0 must not panic and should return empty string
        let (r, m) = scroll_display("hello world", 0, 0);
        assert_eq!(r, "");
        assert_eq!(m, 0);
    }

    #[test]
    fn scroll_start_cjk_boundary_padding() {
        // "日本語テスト" = 12 cols, max_width = 5
        // eff==0 branch: take_columns(s, 4) → 日(2)+本(2)=4, actual_vis=4
        // Result: "日本…" = 5 cols — no padding needed here
        let (r, _) = scroll_display("日本語テスト", 5, 0);
        assert_eq!(display_width(&r), 5);
        // max_width = 4: take_columns(s, 3) → 日(2), actual_vis=2, pad 1
        // Result: "日 …" = 4 cols
        let (r2, _) = scroll_display("日本語テスト", 4, 0);
        assert_eq!(display_width(&r2), 4);
        assert_eq!(r2, "日 …");
    }
}
