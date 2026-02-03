//! Pager-based UI components.
//!
//! This module provides pager functionality for displaying scrollable content,
//! help browser, history browser, and history schema viewer.

mod help;
pub mod history_browser;
pub mod history_schema;
pub mod session_info;
pub(crate) mod text_utils;

pub use help::run_help_browser;
pub use history_browser::{HistoryBrowserResult, HistoryDbMode, run_history_browser};
pub use session_info::display_session_info;

use base64::{Engine, engine::general_purpose};
use crossterm::{
    Command, ExecutableCommand, cursor,
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        MouseEventKind,
    },
    queue,
    style::Stylize,
    terminal::{
        self, BeginSynchronizedUpdate, ClearType, EndSynchronizedUpdate, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use std::io::{self, Write};
use std::time::{Duration, Instant};

/// Result of handling a key event in the pager.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)] // Variants are part of public API for custom handlers
pub enum PagerAction {
    /// Continue running the pager.
    Continue,
    /// Exit the pager.
    Exit,
    /// Request a redraw.
    Redraw,
}

/// Animation scroll speed in milliseconds per character.
pub(crate) const SCROLL_INTERVAL_MS: u64 = 150;

/// Pause duration at the start of scroll animation (in ms).
pub(crate) const SCROLL_PAUSE_MS: u64 = 1000;

/// Run a closure inside an alternate screen with mouse capture and raw mode.
///
/// Handles setup (`EnterAlternateScreen`, `EnableMouseCapture`,
/// `enable_raw_mode`) and guaranteed teardown regardless of the closure result.
pub fn with_alternate_screen<R, F>(f: F) -> io::Result<R>
where
    F: FnOnce() -> io::Result<R>,
{
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    stdout.execute(EnableMouseCapture)?;
    terminal::enable_raw_mode()?;

    let result = f();

    terminal::disable_raw_mode()?;
    stdout.execute(DisableMouseCapture)?;
    stdout.execute(cursor::Show)?;
    stdout.execute(LeaveAlternateScreen)?;

    result
}

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
    /// Get the total number of lines.
    fn line_count(&self) -> usize;

    /// Render a single line at the given index.
    /// Returns the styled string to display.
    fn render_line(&self, index: usize, width: usize) -> String;

    /// Called before rendering to allow content to prepare state.
    /// `scroll_offset` is the first visible line index.
    fn prepare_render(&mut self, _scroll_offset: usize) {}

    /// Handle a custom key event. Return `Some(PagerAction)` if handled.
    fn handle_key(&mut self, _code: KeyCode, _modifiers: KeyModifiers) -> Option<PagerAction> {
        None
    }

    /// Get optional feedback message to display.
    fn feedback_message(&self) -> Option<&str> {
        None
    }

    /// Clear feedback message after display.
    fn clear_feedback(&mut self) {}
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
    let mut stdout = io::stdout();
    let mut scroll_offset = 0;
    let mut needs_redraw = true;

    loop {
        if needs_redraw {
            content.prepare_render(scroll_offset);
            render(&mut stdout, content, config, scroll_offset)?;
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
                            let max_offset = max_scroll_offset(content.line_count());
                            if scroll_offset < max_offset {
                                scroll_offset += 1;
                            }
                        }

                        // Page up
                        (KeyCode::PageUp, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                            let page_size = content_rows();
                            scroll_offset = scroll_offset.saturating_sub(page_size);
                        }

                        // Page down
                        (KeyCode::PageDown, _)
                        | (KeyCode::Char(' '), KeyModifiers::NONE)
                        | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                            let page_size = content_rows();
                            let max_offset = max_scroll_offset(content.line_count());
                            scroll_offset = (scroll_offset + page_size).min(max_offset);
                        }

                        // Home
                        (KeyCode::Home, _) | (KeyCode::Char('g'), KeyModifiers::NONE) => {
                            scroll_offset = 0;
                        }

                        // End
                        (KeyCode::End, _) | (KeyCode::Char('G'), KeyModifiers::SHIFT) => {
                            scroll_offset = max_scroll_offset(content.line_count());
                        }

                        _ => {}
                    }
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp => {
                        if scroll_offset > 0 {
                            scroll_offset -= 1;
                            needs_redraw = true;
                        }
                    }
                    MouseEventKind::ScrollDown => {
                        let max_offset = max_scroll_offset(content.line_count());
                        if scroll_offset < max_offset {
                            scroll_offset += 1;
                            needs_redraw = true;
                        }
                    }
                    _ => {}
                },
                Event::Resize(_, _) => {
                    needs_redraw = true;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Render the pager content.
fn render<C: PagerContent>(
    stdout: &mut io::Stdout,
    content: &C,
    config: &PagerConfig,
    scroll_offset: usize,
) -> io::Result<()> {
    // Begin synchronized update to prevent flickering
    queue!(stdout, BeginSynchronizedUpdate)?;

    stdout.execute(cursor::MoveTo(0, 0))?;
    stdout.execute(cursor::Hide)?;

    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let width = cols as usize;
    let visible_rows = content_rows_with_height(rows as usize);

    // Header
    let header = format!(
        "─ {} [{}/{}] ─",
        config.title,
        scroll_offset + 1,
        content.line_count().max(1)
    );
    let padded_header = format!("{:─<width$}", header, width = width);
    stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
    println!("\r{}", padded_header.dark_grey());

    // Content
    for i in 0..visible_rows {
        stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
        let line_idx = scroll_offset + i;
        if line_idx < content.line_count() {
            let line = content.render_line(line_idx, width);
            println!("\r{}", line);
        } else {
            println!("\r");
        }
    }

    // Footer
    stdout.execute(terminal::Clear(ClearType::CurrentLine))?;
    let footer = if let Some(msg) = content.feedback_message() {
        format!("─ {} ─", msg)
    } else {
        format!("─ {} ─", config.footer_hint)
    };
    let padded_footer = format!("{:─<width$}", footer, width = width);
    println!("\r{}", padded_footer.dark_grey());

    // End synchronized update
    queue!(stdout, EndSynchronizedUpdate)?;
    stdout.flush()?;
    Ok(())
}

/// Get the number of content rows available.
fn content_rows() -> usize {
    let (_, rows) = terminal::size().unwrap_or((80, 24));
    content_rows_with_height(rows as usize)
}

/// Get the number of content rows for a given terminal height.
fn content_rows_with_height(height: usize) -> usize {
    // Reserve 2 lines for header and footer
    height.saturating_sub(2)
}

/// Calculate max scroll offset for given line count.
fn max_scroll_offset(line_count: usize) -> usize {
    line_count.saturating_sub(content_rows())
}

/// OSC 52 clipboard command for copying text via terminal escape sequence.
///
/// Reference: <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Operating-System-Commands>
/// Based on television's implementation: refs/television/television/utils/clipboard.rs
struct SetClipboard {
    content: String,
}

impl SetClipboard {
    fn new(content: &str) -> Self {
        Self {
            content: general_purpose::STANDARD.encode(content.as_bytes()),
        }
    }
}

impl Command for SetClipboard {
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
        write!(f, "\x1b]52;c;{}\x1b\\", self.content)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        // OSC 52 is ANSI-based, no WinAPI implementation needed.
        // Modern Windows terminals support ANSI sequences.
        Ok(())
    }
}

/// Copy text to clipboard using OSC 52 escape sequence.
///
/// This uses the terminal's OSC 52 support to copy text to the system clipboard.
/// Most modern terminals support this (iTerm2, kitty, WezTerm, Windows Terminal, etc.).
pub fn copy_to_clipboard(text: &str) -> io::Result<()> {
    use std::io::BufWriter;
    let mut writer = BufWriter::new(io::stderr());
    crossterm::execute!(writer, SetClipboard::new(text))
}
