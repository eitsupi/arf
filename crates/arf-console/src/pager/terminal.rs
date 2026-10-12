//! Terminal lifecycle and clipboard helpers shared by the pager UIs.

use base64::{Engine, engine::general_purpose};
use crossterm::{
    Command, ExecutableCommand, cursor,
    event::{DisableMouseCapture, EnableMouseCapture},
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::io;

/// RAII guard that restores terminal state on drop, ensuring cleanup even on panic.
struct AlternateScreenGuard;

impl Drop for AlternateScreenGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
        let mut stdout = io::stdout();
        let _ = stdout.execute(DisableMouseCapture);
        let _ = stdout.execute(cursor::Show);
        let _ = stdout.execute(LeaveAlternateScreen);
    }
}

/// Run a closure inside an alternate screen with mouse capture and raw mode.
///
/// Handles setup (`EnterAlternateScreen`, `EnableMouseCapture`,
/// `enable_raw_mode`) and guaranteed teardown via an RAII drop guard,
/// ensuring the terminal is restored even if the closure panics.
pub fn with_alternate_screen<R, F>(f: F) -> io::Result<R>
where
    F: FnOnce() -> io::Result<R>,
{
    // Create the guard *before* setup so that any failure mid-setup still
    // tears down whatever was already enabled (the individual restore calls
    // are no-ops / harmless when the corresponding setup never ran).
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let _guard = AlternateScreenGuard;

    stdout.execute(EnableMouseCapture)?;
    terminal::enable_raw_mode()?;

    f()
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
