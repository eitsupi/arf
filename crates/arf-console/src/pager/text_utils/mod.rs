//! Display-width-aware text utilities for terminal rendering.
//!
//! All width calculations use display columns (not character count), so
//! full-width characters (CJK, some emoji) correctly occupy 2 columns.

use unicode_width::UnicodeWidthStr;

mod wrap;

#[cfg(test)]
mod tests;

pub(in crate::pager) use wrap::{WrappedLine, wrap_spans_with_ranges};

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
