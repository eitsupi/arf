//! Pager-based UI components.
//!
//! This module provides pager functionality for displaying scrollable content,
//! help browser, history browser, and history schema viewer.

mod changelog;
mod help;
mod help_content;
mod help_session;
pub mod history_browser;
pub mod history_schema;
mod lines_content;
pub(crate) mod markdown;
pub mod session_info;
mod size;
pub(crate) mod style_convert;
mod terminal;
pub(crate) mod text_utils;

pub use changelog::display_changelog;
pub(crate) use help::display_prepared_help_request;
pub use help::run_help_browser;
pub use history_browser::{HistoryBrowserResult, HistoryDbMode, run_history_browser};
pub use session_info::display_session_info;
pub(crate) use size::{
    MinimumSize, check_terminal_too_small, render_size_warning_if_too_small, visible_result_rows,
    visible_result_rows_for,
};
pub use terminal::{copy_to_clipboard, with_alternate_screen};

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color as RatColor, Style as RatStyle};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::io;
use std::time::{Duration, Instant};

/// Result of handling a key event in the pager.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Variants are part of public API for custom handlers
pub enum PagerAction {
    /// Continue running the pager, including its standard key handling.
    Continue,
    /// Exit the pager.
    Exit,
    /// Consume the key and request a redraw without standard key handling.
    Redraw,
    /// Consume the key and move to a line, clamped to the valid scroll range.
    ScrollTo(usize),
}

/// Animation scroll speed in milliseconds per character.
pub(crate) const SCROLL_INTERVAL_MS: u64 = 150;

/// Pause duration at the start of scroll animation (in ms).
pub(crate) const SCROLL_PAUSE_MS: u64 = 1000;

/// Manages text scroll animation state for long items in a list view.
///
/// Call [`update`](Self::update) each frame with the current cursor index.
/// When the cursor stays on the same item, the animation ticks forward
/// after an initial pause, returning `true` when a redraw is needed.
pub(crate) struct TextScrollState {
    /// Current horizontal scroll position in display columns.
    pub scroll_pos: usize,
    prev_cursor: usize,
    scroll_start: Instant,
}

impl TextScrollState {
    pub fn new() -> Self {
        Self {
            scroll_pos: 0,
            prev_cursor: usize::MAX,
            scroll_start: Instant::now(),
        }
    }

    /// Advance the animation. Returns `true` if the display changed.
    ///
    /// `current_cursor` is the index of the currently highlighted item.
    pub fn update(&mut self, current_cursor: usize) -> bool {
        if current_cursor != self.prev_cursor {
            self.prev_cursor = current_cursor;
            self.scroll_pos = 0;
            self.scroll_start = Instant::now();
            return true;
        }

        let elapsed = self.scroll_start.elapsed();

        if elapsed < Duration::from_millis(SCROLL_PAUSE_MS) {
            return false;
        }

        let scroll_time = elapsed - Duration::from_millis(SCROLL_PAUSE_MS);
        let new_pos = (scroll_time.as_millis() / SCROLL_INTERVAL_MS as u128) as usize;

        if new_pos != self.scroll_pos {
            self.scroll_pos = new_pos;
            true
        } else {
            false
        }
    }
}

/// Configuration for the pager.
pub struct PagerConfig<'a> {
    /// Title displayed in the header.
    pub title: &'a str,
    /// Footer hint text (e.g., "q to quit").
    pub footer_hint: &'a str,
    /// Whether the pager manages its own alternate screen.
    /// Set to false if already in alternate screen (e.g., help browser).
    pub manage_alternate_screen: bool,
}

impl<'a> Default for PagerConfig<'a> {
    fn default() -> Self {
        Self {
            title: "Pager",
            footer_hint: "↑↓/jk scroll  q/Esc exit",
            manage_alternate_screen: true,
        }
    }
}

/// Trait for content that can be displayed in the pager.
pub trait PagerContent {
    /// Override the configured header for content that changes pages.
    fn title(&self) -> Option<&str> {
        None
    }

    /// Get the total number of lines.
    fn line_count(&self) -> usize;

    /// Render a single line at the given index.
    /// Returns a styled `Line` for ratatui rendering.
    fn render_line(&self, index: usize, width: usize) -> Line<'static>;

    /// Called before rendering to allow content to prepare state.
    /// `scroll_offset` is the first visible line index.
    fn prepare_render(&mut self, _scroll_offset: usize) {}

    /// Request a viewport change after a non-key transition such as resize.
    /// The pager consumes this request and clamps it before rendering.
    fn take_scroll_request(&mut self) -> Option<usize> {
        None
    }

    /// Handle a custom key event. `None` and `Continue` use standard key handling;
    /// `Redraw` and `ScrollTo` consume the key.
    fn handle_key(&mut self, _code: KeyCode, _modifiers: KeyModifiers) -> Option<PagerAction> {
        None
    }

    /// Get optional feedback message to display.
    fn feedback_message(&self) -> Option<&str> {
        None
    }

    /// Clear feedback message after display.
    fn clear_feedback(&mut self) {}

    /// Called when the terminal is resized.
    ///
    /// Implementations can use this to re-render width-dependent content
    /// (e.g., re-wrap Markdown text). The pager always redraws and clamps
    /// the scroll position after this notification.
    fn on_resize(&mut self, _width: usize, _height: usize) {}
}

/// Run the pager with the given content and configuration.
pub fn run<C: PagerContent>(content: &mut C, config: &PagerConfig) -> io::Result<()> {
    if config.manage_alternate_screen {
        with_alternate_screen(|| run_inner(content, config))
    } else {
        run_inner(content, config)
    }
}

/// Inner pager loop.
fn run_inner<C: PagerContent>(content: &mut C, config: &PagerConfig) -> io::Result<()> {
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    // Clear both the internal buffer and the screen so ratatui doesn't
    // assume the screen is empty and skip cells that still show content
    // from a previous UI (e.g., help browser list behind the help pager).
    terminal.clear()?;
    terminal.hide_cursor()?;
    let mut scroll_offset = 0;
    let mut needs_redraw = true;
    let (_, initial_rows) = crossterm::terminal::size().unwrap_or((80, 24));
    let mut term_height = initial_rows as usize;

    loop {
        if needs_redraw {
            if let Some(line) = content.take_scroll_request() {
                scroll_offset = clamp_scroll_offset(line, content.line_count(), term_height);
            }
            content.prepare_render(scroll_offset);
            render(&mut terminal, content, config, scroll_offset)?;
            needs_redraw = false;
        }

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Key(key) => {
                    // Only handle key press events, ignore release and repeat
                    // This is important on Windows where release events are sent
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }

                    // Clear feedback message on any key press
                    content.clear_feedback();
                    needs_redraw = true;

                    // Let content handle custom keys first
                    if let Some(action) = content.handle_key(key.code, key.modifiers) {
                        match action {
                            PagerAction::Exit => break,
                            PagerAction::Redraw => continue,
                            PagerAction::ScrollTo(line) => {
                                scroll_offset =
                                    clamp_scroll_offset(line, content.line_count(), term_height);
                                continue;
                            }
                            PagerAction::Continue => {}
                        }
                    }

                    // Standard navigation
                    match (key.code, key.modifiers) {
                        // Exit
                        (KeyCode::Esc, _)
                        | (KeyCode::Char('q'), KeyModifiers::NONE)
                        | (KeyCode::Char('c'), KeyModifiers::CONTROL)
                        | (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                            break;
                        }

                        // Navigation - up
                        (KeyCode::Up, _)
                        | (KeyCode::Char('k'), KeyModifiers::NONE)
                        | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                            scroll_offset = scroll_offset.saturating_sub(1);
                        }

                        // Navigation - down
                        (KeyCode::Down, _)
                        | (KeyCode::Char('j'), KeyModifiers::NONE)
                        | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
                            let max_offset =
                                max_scroll_offset_with_height(content.line_count(), term_height);
                            if scroll_offset < max_offset {
                                scroll_offset += 1;
                            }
                        }

                        // Page up
                        (KeyCode::PageUp, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                            let page_size = content_rows_with_height(term_height);
                            scroll_offset = scroll_offset.saturating_sub(page_size);
                        }

                        // Page down
                        (KeyCode::PageDown, _)
                        | (KeyCode::Char(' '), KeyModifiers::NONE)
                        | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                            let page_size = content_rows_with_height(term_height);
                            let max_offset =
                                max_scroll_offset_with_height(content.line_count(), term_height);
                            scroll_offset = (scroll_offset + page_size).min(max_offset);
                        }

                        // Home
                        (KeyCode::Home, _) | (KeyCode::Char('g'), KeyModifiers::NONE) => {
                            scroll_offset = 0;
                        }

                        // End
                        (KeyCode::End, _) | (KeyCode::Char('G'), KeyModifiers::SHIFT) => {
                            scroll_offset =
                                max_scroll_offset_with_height(content.line_count(), term_height);
                        }

                        _ => {}
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        needs_redraw = true;
                        scroll_offset = scroll_offset.saturating_sub(1);
                    }
                    MouseEventKind::ScrollDown => {
                        needs_redraw = true;
                        let max_offset =
                            max_scroll_offset_with_height(content.line_count(), term_height);
                        if scroll_offset < max_offset {
                            scroll_offset += 1;
                        }
                    }
                    _ => {}
                },
                Event::Resize(w, h) => {
                    term_height = h as usize;
                    content.on_resize(w as usize, term_height);
                    // Height changes also change the valid range without re-wrapping.
                    scroll_offset =
                        clamp_scroll_offset(scroll_offset, content.line_count(), term_height);
                    needs_redraw = true;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Render the pager content using ratatui.
fn render<C: PagerContent>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    content: &C,
    config: &PagerConfig,
    scroll_offset: usize,
) -> io::Result<()> {
    terminal.draw(|frame| {
        let area = frame.area();
        let width = area.width as usize;

        // Layout: header(1) + content(Fill) + footer(1)
        let [header_area, content_area, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(area);

        // Header
        let header_text = format!(
            "─ {} [{}/{}] ─",
            content.title().unwrap_or(config.title),
            scroll_offset + 1,
            content.line_count().max(1)
        );
        let padded_header = format!("{:─<width$}", header_text, width = width);
        let header = Paragraph::new(Span::styled(
            padded_header,
            RatStyle::default().fg(RatColor::DarkGray),
        ));
        frame.render_widget(header, header_area);

        // Content
        let visible_rows = content_area.height as usize;
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible_rows);
        for i in 0..visible_rows {
            let line_idx = scroll_offset + i;
            if line_idx < content.line_count() {
                let mut line = content.render_line(line_idx, width);
                // Pad lines with background color (e.g. code blocks) to fill full width.
                // ratatui's Paragraph only applies Line.style bg to the content width,
                // so we extend with spaces to make the background span the entire row.
                if line.style.bg.is_some() {
                    let current_w: usize = line.spans.iter().map(|s| s.width()).sum();
                    if current_w < width {
                        line.spans.push(Span::raw(" ".repeat(width - current_w)));
                    }
                }
                lines.push(line);
            } else {
                lines.push(Line::from(""));
            }
        }
        let content_widget = Paragraph::new(lines);
        frame.render_widget(content_widget, content_area);

        // Footer
        let footer_text = if let Some(msg) = content.feedback_message() {
            format!("─ {} ─", msg)
        } else {
            format!("─ {} ─", config.footer_hint)
        };
        let padded_footer = format!("{:─<width$}", footer_text, width = width);
        let footer = Paragraph::new(Span::styled(
            padded_footer,
            RatStyle::default().fg(RatColor::DarkGray),
        ));
        frame.render_widget(footer, footer_area);
    })?;

    Ok(())
}

/// Get the number of content rows for a given terminal height.
fn content_rows_with_height(height: usize) -> usize {
    // Reserve 2 lines for header and footer
    height.saturating_sub(2)
}

/// Calculate max scroll offset for given line count and terminal height.
fn max_scroll_offset_with_height(line_count: usize, height: usize) -> usize {
    line_count.saturating_sub(content_rows_with_height(height))
}

fn clamp_scroll_offset(requested: usize, line_count: usize, height: usize) -> usize {
    requested.min(max_scroll_offset_with_height(line_count, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_scroll_requests_are_clamped_to_the_viewport_range() {
        assert_eq!(clamp_scroll_offset(usize::MAX, 100, 24), 78);
        assert_eq!(clamp_scroll_offset(10, 100, 24), 10);
        assert_eq!(clamp_scroll_offset(20, 5, 24), 0);
        assert_eq!(clamp_scroll_offset(1, 0, 24), 0);
        assert_eq!(clamp_scroll_offset(78, 100, 50), 52);
    }
}
