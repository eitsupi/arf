#![allow(dead_code)]
//! Common test utilities for arf integration tests.
//!
//! This module provides a PTY-based testing infrastructure that properly handles
//! cursor position queries (CSI 6n) from reedline. This is essential for testing
//! interactive features of arf.
//!
//! # Platform Support
//!
//! This module is Unix-only because crossterm's `cursor::position()` uses WinAPI
//! on Windows, which doesn't work correctly inside ConPTY. This matches crossterm's
//! own testing approach where cursor position tests are marked `#[ignore]` in CI.
//!
//! See: <https://github.com/crossterm-rs/crossterm/blob/master/src/cursor.rs>
//!
//! # Architecture
//!
//! The Terminal struct uses:
//! - `portable-pty` for cross-platform PTY management
//! - `vt100` for terminal emulation and cursor query detection
//! - Separate threads for reading PTY output and handling cursor queries
//!
//! # Assertion Style (inspired by radian)
//!
//! This module provides screen-based assertions similar to radian's test framework:
//! - `line(n)` - Get line n from the screen
//! - `current_line()` - Get the line at cursor position
//! - `previous_line(n)` - Get line n lines above cursor
//! - `cursor_position()` - Get cursor (row, col)
//!
//! Each returns a `ScreenLine` that supports fluent assertions with timeouts:
//! - `assert_startswith("prefix")` - Assert line starts with prefix
//! - `assert_endswith("suffix")` - Assert line ends with suffix
//! - `assert_contains("substring")` - Assert line contains substring
//! - `assert_equal("exact")` - Assert line equals exact string
#![cfg(unix)]

use portable_pty::{Child, CommandBuilder, PtySize, native_pty_system};
use regex::Regex;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Default timeout for waiting on terminal output (in milliseconds).
const DEFAULT_TIMEOUT_MS: u64 = 15000;

/// Default terminal size.
const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;

/// Screen state snapshot for assertions.
#[derive(Clone)]
pub struct ScreenSnapshot {
    /// Lines of the screen (row 0 at top)
    pub lines: Vec<String>,
    /// Cursor row (0-indexed)
    pub cursor_row: u16,
    /// Cursor column (0-indexed)
    pub cursor_col: u16,
}

/// Shared state between threads.
struct SharedState {
    /// Accumulated output from the terminal (raw bytes as string)
    output_buffer: String,
    /// Whether the terminal is still running
    running: bool,
    /// Current screen snapshot from vt100
    screen: ScreenSnapshot,
}

/// Terminal wrapper for integration testing.
///
/// Provides methods to interact with a PTY-spawned arf process
/// and assert on its output. This implementation properly handles
/// cursor position queries from reedline.
pub struct Terminal {
    /// Shared state protected by mutex
    state: Arc<Mutex<SharedState>>,
    /// Handle to write to the PTY
    pty_writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Handle to the reader thread
    _reader_handle: JoinHandle<()>,
    /// Handle to the child process
    child: Box<dyn Child + Send + Sync>,
    /// Flag to signal shutdown
    shutdown: Arc<AtomicBool>,
}

impl Terminal {
    /// Spawn arf in a PTY and return a Terminal handle.
    pub fn spawn() -> Result<Self, String> {
        Self::spawn_with_args(&[])
    }

    /// Spawn arf with additional arguments.
    pub fn spawn_with_args(args: &[&str]) -> Result<Self, String> {
        let bin_path = env!("CARGO_BIN_EXE_arf");

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: DEFAULT_ROWS,
                cols: DEFAULT_COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Failed to open PTY: {}", e))?;

        // Build command
        let mut cmd = CommandBuilder::new(bin_path);
        // Disable history by default to avoid writing to user's actual history file during tests.
        // Skip this if --history-dir is explicitly provided (for history-related tests).
        let has_history_dir = args.contains(&"--history-dir");
        if !has_history_dir {
            cmd.arg("--no-history");
        }
        for arg in args {
            cmd.arg(*arg);
        }

        // Spawn the process
        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Failed to spawn arf: {}", e))?;

        // Get master reader and writer
        let pty_writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("Failed to get PTY writer: {}", e))?;
        let mut pty_reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("Failed to get PTY reader: {}", e))?;

        // Drop slave end in parent process
        drop(pair.slave);

        // Shared state
        let state = Arc::new(Mutex::new(SharedState {
            output_buffer: String::new(),
            running: true,
            screen: ScreenSnapshot {
                lines: vec![String::new(); DEFAULT_ROWS as usize],
                cursor_row: 0,
                cursor_col: 0,
            },
        }));

        // Shared writer for responding to cursor queries
        let pty_writer = Arc::new(Mutex::new(pty_writer));
        let pty_writer_clone = Arc::clone(&pty_writer);

        // Shutdown flag
        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);

        // State for reader thread
        let state_clone = Arc::clone(&state);

        // Spawn reader thread that handles PTY I/O
        let reader_handle = thread::spawn(move || {
            // vt100 parser for cursor query detection
            let (query_tx, query_rx) = std::sync::mpsc::channel::<()>();

            struct CursorQueryDetector {
                query_tx: std::sync::mpsc::Sender<()>,
            }

            impl vt100::Callbacks for CursorQueryDetector {
                fn unhandled_csi(
                    &mut self,
                    _screen: &mut vt100::Screen,
                    _prefix: Option<u8>,
                    _intermediate: Option<u8>,
                    params: &[&[u16]],
                    c: char,
                ) {
                    if c == 'n' {
                        let is_dsr = params.is_empty()
                            || (params.len() == 1 && params[0].len() == 1 && params[0][0] == 6);
                        if is_dsr {
                            let _ = self.query_tx.send(());
                        }
                    }
                }
            }

            let callbacks = CursorQueryDetector { query_tx };
            let mut parser =
                vt100::Parser::new_with_callbacks(DEFAULT_ROWS, DEFAULT_COLS, 0, callbacks);

            let mut buf = [0u8; 4096];

            loop {
                if shutdown_clone.load(Ordering::Relaxed) {
                    break;
                }

                // Read from PTY (this will block until data is available)
                match pty_reader.read(&mut buf) {
                    Ok(0) => {
                        // EOF - process exited
                        if let Ok(mut state) = state_clone.lock() {
                            state.running = false;
                        }
                        break;
                    }
                    Ok(n) => {
                        let data = &buf[..n];

                        // Process through vt100 parser
                        parser.process(data);

                        // Update shared state
                        if let Ok(mut state) = state_clone.lock() {
                            // Add to raw output buffer
                            if let Ok(s) = std::str::from_utf8(data) {
                                state.output_buffer.push_str(s);
                            }

                            // Update screen snapshot from vt100
                            let screen = parser.screen();
                            let (cursor_row, cursor_col) = screen.cursor_position();
                            state.screen.cursor_row = cursor_row;
                            state.screen.cursor_col = cursor_col;
                            for row in 0..DEFAULT_ROWS {
                                let row_content =
                                    screen.contents_between(row, 0, row, DEFAULT_COLS - 1);
                                state.screen.lines[row as usize] = row_content;
                            }
                        }

                        // Check for cursor queries and respond
                        while query_rx.try_recv().is_ok() {
                            let (row, col) = parser.screen().cursor_position();
                            let response = format!("\x1b[{};{}R", row + 1, col + 1);
                            if let Ok(mut writer) = pty_writer_clone.lock() {
                                let _ = writer.write_all(response.as_bytes());
                                let _ = writer.flush();
                            }
                        }
                    }
                    Err(e) => {
                        if e.kind() != std::io::ErrorKind::WouldBlock
                            && e.kind() != std::io::ErrorKind::Interrupted
                        {
                            // Real error
                            if let Ok(mut state) = state_clone.lock() {
                                state.running = false;
                            }
                            break;
                        }
                    }
                }
            }
        });

        Ok(Terminal {
            state,
            pty_writer,
            _reader_handle: reader_handle,
            child,
            shutdown,
        })
    }

    /// Wait for a string pattern in the output with timeout.
    pub fn expect(&mut self, pattern: &str) -> Result<(), String> {
        let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
        let start = Instant::now();

        while start.elapsed() < timeout {
            // Check if still running
            {
                let state = self.state.lock().map_err(|e| e.to_string())?;
                if !state.running && !state.output_buffer.contains(pattern) {
                    return Err(format!(
                        "Process exited before finding pattern '{}'. Output:\n{}",
                        pattern, state.output_buffer
                    ));
                }
                if state.output_buffer.contains(pattern) {
                    return Ok(());
                }
            }

            thread::sleep(Duration::from_millis(50));
        }

        // Timeout - get final output for error message
        let output = self
            .state
            .lock()
            .map(|s| s.output_buffer.clone())
            .unwrap_or_default();
        Err(format!(
            "Timeout waiting for pattern '{}'. Current output:\n{}",
            pattern, output
        ))
    }

    /// Wait for a regex pattern in the output.
    #[allow(dead_code)]
    pub fn expect_regex(&mut self, pattern: &str) -> Result<(), String> {
        let re = Regex::new(pattern).map_err(|e| format!("Invalid regex: {}", e))?;
        let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
        let start = Instant::now();

        while start.elapsed() < timeout {
            {
                let state = self.state.lock().map_err(|e| e.to_string())?;
                if !state.running && !re.is_match(&state.output_buffer) {
                    return Err(format!(
                        "Process exited before matching regex '{}'. Output:\n{}",
                        pattern, state.output_buffer
                    ));
                }
                if re.is_match(&state.output_buffer) {
                    return Ok(());
                }
            }

            thread::sleep(Duration::from_millis(50));
        }

        let output = self
            .state
            .lock()
            .map(|s| s.output_buffer.clone())
            .unwrap_or_default();
        Err(format!(
            "Timeout waiting for regex '{}'. Current output:\n{}",
            pattern, output
        ))
    }

    /// Wait for the prompt to appear.
    /// Looks for "> " which matches both "r> " and "R {version}> " prompts.
    pub fn wait_for_prompt(&mut self) -> Result<(), String> {
        self.expect("> ")
    }

    /// Clear the output buffer and wait for new output matching pattern.
    ///
    /// This is useful when you need to wait for output from a specific command,
    /// not just any matching text that might already be in the buffer.
    pub fn clear_and_expect(&mut self, pattern: &str) -> Result<(), String> {
        // Clear the buffer
        {
            let mut state = self.state.lock().map_err(|e| e.to_string())?;
            state.output_buffer.clear();
        }

        // Now wait for the pattern
        self.expect(pattern)
    }

    /// Clear the output buffer.
    pub fn clear_buffer(&mut self) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        state.output_buffer.clear();
        Ok(())
    }

    /// Get the current output buffer contents.
    pub fn get_output(&self) -> Result<String, String> {
        let state = self.state.lock().map_err(|e| e.to_string())?;
        Ok(state.output_buffer.clone())
    }

    /// Send a line of input (appends newline).
    pub fn send_line(&mut self, text: &str) -> Result<(), String> {
        let data = format!("{}\n", text);
        let mut writer = self.pty_writer.lock().map_err(|e| e.to_string())?;
        writer
            .write_all(data.as_bytes())
            .map_err(|e| format!("Failed to send line: {}", e))?;
        writer
            .flush()
            .map_err(|e| format!("Failed to flush: {}", e))
    }

    /// Send raw text without newline.
    pub fn send(&mut self, text: &str) -> Result<(), String> {
        let mut writer = self.pty_writer.lock().map_err(|e| e.to_string())?;
        writer
            .write_all(text.as_bytes())
            .map_err(|e| format!("Failed to send: {}", e))?;
        writer
            .flush()
            .map_err(|e| format!("Failed to flush: {}", e))
    }

    /// Send Ctrl+C (interrupt).
    pub fn send_interrupt(&mut self) -> Result<(), String> {
        self.send("\x03")
    }

    /// Send Ctrl+D (EOF).
    pub fn send_eof(&mut self) -> Result<(), String> {
        self.send("\x04")
    }

    /// Gracefully quit arf.
    pub fn quit(&mut self) -> Result<(), String> {
        // Try q() first
        let _ = self.send_line("q()");
        thread::sleep(Duration::from_millis(500));

        // If still alive, send Ctrl+D
        {
            let state = self.state.lock().map_err(|e| e.to_string())?;
            if state.running {
                drop(state);
                let _ = self.send_eof();
            }
        }

        // Signal shutdown
        self.shutdown.store(true, Ordering::Relaxed);

        // Kill child if still running
        let _ = self.child.kill();

        Ok(())
    }

    // ========================================================================
    // Screen-based assertions (inspired by radian's test framework)
    // ========================================================================

    /// Get the current screen snapshot.
    pub fn screen(&self) -> Result<ScreenSnapshot, String> {
        let state = self.state.lock().map_err(|e| e.to_string())?;
        Ok(state.screen.clone())
    }

    /// Get a specific line from the screen (0-indexed).
    pub fn line(&self, row: usize) -> ScreenLine {
        ScreenLine {
            state: Arc::clone(&self.state),
            line_getter: LineGetter::Absolute(row),
        }
    }

    /// Get the line at the current cursor position.
    pub fn current_line(&self) -> ScreenLine {
        ScreenLine {
            state: Arc::clone(&self.state),
            line_getter: LineGetter::CurrentLine,
        }
    }

    /// Get a line relative to the cursor (n lines above).
    pub fn previous_line(&self, n: usize) -> ScreenLine {
        ScreenLine {
            state: Arc::clone(&self.state),
            line_getter: LineGetter::PreviousLine(n),
        }
    }

    /// Get current cursor position as (row, col), both 0-indexed.
    pub fn cursor_position(&self) -> Result<(u16, u16), String> {
        let state = self.state.lock().map_err(|e| e.to_string())?;
        Ok((state.screen.cursor_row, state.screen.cursor_col))
    }

    /// Assert cursor is at a specific position (row, col), both 0-indexed.
    pub fn assert_cursor(&self, expected_row: u16, expected_col: u16) -> Result<(), String> {
        let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
        let start = Instant::now();

        while start.elapsed() < timeout {
            let (row, col) = self.cursor_position()?;
            if row == expected_row && col == expected_col {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }

        let (row, col) = self.cursor_position()?;
        Err(format!(
            "Cursor position mismatch: expected ({}, {}), got ({}, {})",
            expected_row, expected_col, row, col
        ))
    }

    /// Debug helper: print current screen state.
    #[allow(dead_code)]
    pub fn dump_screen(&self) -> Result<(), String> {
        let state = self.state.lock().map_err(|e| e.to_string())?;
        eprintln!("=== Screen Dump ===");
        eprintln!(
            "Cursor: ({}, {})",
            state.screen.cursor_row, state.screen.cursor_col
        );
        for (i, line) in state.screen.lines.iter().enumerate() {
            let trimmed = line.trim_end();
            if !trimmed.is_empty() || i == state.screen.cursor_row as usize {
                let marker = if i == state.screen.cursor_row as usize {
                    ">"
                } else {
                    " "
                };
                eprintln!("{} {:2}: {:?}", marker, i, trimmed);
            }
        }
        eprintln!("===================");
        Ok(())
    }
}

/// How to get a line from the screen.
#[allow(dead_code)]
enum LineGetter {
    /// Get line at absolute row index (0-indexed)
    Absolute(usize),
    /// Get line at cursor position
    CurrentLine,
    /// Get line N rows above cursor
    PreviousLine(usize),
}

/// A reference to a screen line that supports fluent assertions.
pub struct ScreenLine {
    state: Arc<Mutex<SharedState>>,
    line_getter: LineGetter,
}

impl ScreenLine {
    /// Get the current line content.
    fn get_line(&self) -> Result<String, String> {
        let state = self.state.lock().map_err(|e| e.to_string())?;
        let row = match self.line_getter {
            LineGetter::Absolute(r) => r,
            LineGetter::CurrentLine => state.screen.cursor_row as usize,
            LineGetter::PreviousLine(n) => (state.screen.cursor_row as usize).saturating_sub(n),
        };
        Ok(state.screen.lines.get(row).cloned().unwrap_or_default())
    }

    /// Assert line starts with prefix (with timeout).
    #[allow(dead_code)]
    pub fn assert_startswith(&self, prefix: &str) -> Result<(), String> {
        let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
        let start = Instant::now();

        while start.elapsed() < timeout {
            let line = self.get_line()?;
            if line.starts_with(prefix) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }

        let line = self.get_line()?;
        Err(format!(
            "Line does not start with '{}': got '{}'",
            prefix, line
        ))
    }

    /// Assert line ends with suffix (with timeout).
    #[allow(dead_code)]
    pub fn assert_endswith(&self, suffix: &str) -> Result<(), String> {
        let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
        let start = Instant::now();

        while start.elapsed() < timeout {
            let line = self.get_line()?;
            if line.trim_end().ends_with(suffix) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }

        let line = self.get_line()?;
        Err(format!(
            "Line does not end with '{}': got '{}'",
            suffix, line
        ))
    }

    /// Assert line contains substring (with timeout).
    pub fn assert_contains(&self, substring: &str) -> Result<(), String> {
        let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
        let start = Instant::now();

        while start.elapsed() < timeout {
            let line = self.get_line()?;
            if line.contains(substring) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }

        let line = self.get_line()?;
        Err(format!(
            "Line does not contain '{}': got '{}'",
            substring, line
        ))
    }

    /// Assert line equals expected (with timeout). Compares trimmed strings.
    #[allow(dead_code)]
    pub fn assert_equal(&self, expected: &str) -> Result<(), String> {
        let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
        let start = Instant::now();

        while start.elapsed() < timeout {
            let line = self.get_line()?;
            if line.trim() == expected.trim() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }

        let line = self.get_line()?;
        Err(format!(
            "Line does not equal '{}': got '{}'",
            expected, line
        ))
    }

    /// Get the trimmed line content (for chaining).
    #[allow(dead_code)]
    pub fn trim(&self) -> TrimmedScreenLine<'_> {
        TrimmedScreenLine { inner: self }
    }
}

/// A trimmed view of a screen line.
pub struct TrimmedScreenLine<'a> {
    inner: &'a ScreenLine,
}

impl TrimmedScreenLine<'_> {
    /// Assert trimmed line starts with prefix.
    #[allow(dead_code)]
    pub fn assert_startswith(&self, prefix: &str) -> Result<(), String> {
        let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
        let start = Instant::now();

        while start.elapsed() < timeout {
            let line = self.inner.get_line()?;
            if line.trim().starts_with(prefix) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }

        let line = self.inner.get_line()?;
        Err(format!(
            "Trimmed line does not start with '{}': got '{}'",
            prefix,
            line.trim()
        ))
    }

    /// Assert trimmed line ends with suffix.
    #[allow(dead_code)]
    pub fn assert_endswith(&self, suffix: &str) -> Result<(), String> {
        let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
        let start = Instant::now();

        while start.elapsed() < timeout {
            let line = self.inner.get_line()?;
            if line.trim().ends_with(suffix) {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }

        let line = self.inner.get_line()?;
        Err(format!(
            "Trimmed line does not end with '{}': got '{}'",
            suffix,
            line.trim()
        ))
    }

    /// Assert trimmed line equals expected.
    #[allow(dead_code)]
    pub fn assert_equal(&self, expected: &str) -> Result<(), String> {
        let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
        let start = Instant::now();

        while start.elapsed() < timeout {
            let line = self.inner.get_line()?;
            if line.trim() == expected {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(50));
        }

        let line = self.inner.get_line()?;
        Err(format!(
            "Trimmed line does not equal '{}': got '{}'",
            expected,
            line.trim()
        ))
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
    }
}

// ============================================================================
// External tool detection helpers
// ============================================================================

use std::process::Command;

/// Check if Air CLI is available on the system.
///
/// Returns true if `air --version` runs successfully.
pub fn has_air_cli() -> bool {
    Command::new("air")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check if dplyr R package is available.
///
/// Returns true if R can load dplyr without error.
pub fn has_dplyr() -> bool {
    Command::new("Rscript")
        .args(["-e", "library(dplyr)"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
