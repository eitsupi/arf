//! Conversion utilities between nu_ansi_term/reedline styles and ratatui styles.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Convert a `nu_ansi_term::Color` to a `ratatui::style::Color`.
pub(crate) fn nu_ansi_color_to_ratatui(color: nu_ansi_term::Color) -> Color {
    match color {
        nu_ansi_term::Color::Black => Color::Black,
        nu_ansi_term::Color::Red => Color::Red,
        nu_ansi_term::Color::Green => Color::Green,
        nu_ansi_term::Color::Yellow => Color::Yellow,
        nu_ansi_term::Color::Blue => Color::Blue,
        nu_ansi_term::Color::Purple | nu_ansi_term::Color::Magenta => Color::Magenta,
        nu_ansi_term::Color::Cyan => Color::Cyan,
        nu_ansi_term::Color::White => Color::White,
        nu_ansi_term::Color::DarkGray => Color::DarkGray,
        nu_ansi_term::Color::LightRed => Color::LightRed,
        nu_ansi_term::Color::LightGreen => Color::LightGreen,
        nu_ansi_term::Color::LightYellow => Color::LightYellow,
        nu_ansi_term::Color::LightBlue => Color::LightBlue,
        nu_ansi_term::Color::LightPurple | nu_ansi_term::Color::LightMagenta => Color::LightMagenta,
        nu_ansi_term::Color::LightCyan => Color::LightCyan,
        nu_ansi_term::Color::LightGray => Color::Gray,
        nu_ansi_term::Color::Fixed(n) => Color::Indexed(n),
        nu_ansi_term::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
        // Default is not a real color; treat as reset
        nu_ansi_term::Color::Default => Color::Reset,
    }
}

/// Convert a `nu_ansi_term::Style` to a `ratatui::style::Style`.
fn nu_ansi_to_ratatui(style: &nu_ansi_term::Style) -> Style {
    let mut s = Style::default();
    if let Some(fg) = style.foreground {
        s = s.fg(nu_ansi_color_to_ratatui(fg));
    }
    if let Some(bg) = style.background {
        s = s.bg(nu_ansi_color_to_ratatui(bg));
    }
    let mut mods = Modifier::empty();
    if style.is_bold {
        mods |= Modifier::BOLD;
    }
    if style.is_italic {
        mods |= Modifier::ITALIC;
    }
    if style.is_underline {
        mods |= Modifier::UNDERLINED;
    }
    if style.is_dimmed {
        mods |= Modifier::DIM;
    }
    if style.is_strikethrough {
        mods |= Modifier::CROSSED_OUT;
    }
    if !mods.is_empty() {
        s = s.add_modifier(mods);
    }
    s
}

/// Convert a `reedline::StyledText` to a `ratatui::text::Line<'static>`.
///
/// Each `(nu_ansi_term::Style, String)` pair in the buffer becomes a `Span`.
pub fn styled_text_to_line(styled: &reedline::StyledText) -> Line<'static> {
    let spans: Vec<Span<'static>> = styled
        .buffer
        .iter()
        .map(|(style, text)| {
            let ratatui_style = nu_ansi_to_ratatui(style);
            Span::styled(text.clone(), ratatui_style)
        })
        .collect();
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nu_ansi_color_to_ratatui_basic() {
        assert_eq!(
            nu_ansi_color_to_ratatui(nu_ansi_term::Color::Red),
            Color::Red
        );
        assert_eq!(
            nu_ansi_color_to_ratatui(nu_ansi_term::Color::Blue),
            Color::Blue
        );
        assert_eq!(
            nu_ansi_color_to_ratatui(nu_ansi_term::Color::DarkGray),
            Color::DarkGray
        );
    }

    #[test]
    fn test_nu_ansi_color_to_ratatui_rgb() {
        assert_eq!(
            nu_ansi_color_to_ratatui(nu_ansi_term::Color::Rgb(10, 20, 30)),
            Color::Rgb(10, 20, 30)
        );
    }

    #[test]
    fn test_nu_ansi_color_to_ratatui_fixed() {
        assert_eq!(
            nu_ansi_color_to_ratatui(nu_ansi_term::Color::Fixed(42)),
            Color::Indexed(42)
        );
    }

    #[test]
    fn test_nu_ansi_to_ratatui_bold_italic() {
        let style = nu_ansi_term::Style::new().bold().italic();
        let ratatui_style = nu_ansi_to_ratatui(&style);
        assert!(ratatui_style.add_modifier.contains(Modifier::BOLD));
        assert!(ratatui_style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn test_nu_ansi_to_ratatui_fg_bg() {
        let style = nu_ansi_term::Style::new()
            .fg(nu_ansi_term::Color::Cyan)
            .on(nu_ansi_term::Color::Black);
        let ratatui_style = nu_ansi_to_ratatui(&style);
        assert_eq!(ratatui_style.fg, Some(Color::Cyan));
        assert_eq!(ratatui_style.bg, Some(Color::Black));
    }

    #[test]
    fn test_styled_text_to_line() {
        let mut styled = reedline::StyledText::new();
        let mut s = nu_ansi_term::Style::new();
        s.foreground = Some(nu_ansi_term::Color::Red);
        styled.buffer.push((s, "hello".to_string()));
        styled
            .buffer
            .push((nu_ansi_term::Style::new(), " world".to_string()));

        let line = styled_text_to_line(&styled);
        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.spans[0].content, "hello");
        assert_eq!(line.spans[0].style.fg, Some(Color::Red));
        assert_eq!(line.spans[1].content, " world");
    }
}
