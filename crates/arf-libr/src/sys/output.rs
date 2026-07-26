use std::os::raw::{c_char, c_int};
use std::sync::RwLock;

use super::error_state::is_stderr_suppressed;
use super::spinner::stop_spinner;

/// Console write callback function pointer storage.
///
/// # Safety
///
/// This static is only accessed from R's main thread: R is single-threaded,
/// and both the `r_write_console_ex` callback and the `set_`/`clear_`
/// functions are called exclusively from that thread. No synchronization
/// primitive is needed as long as this invariant holds. If multi-threaded
/// access ever becomes possible, replace with `AtomicPtr` or similar.
static mut WRITE_CONSOLE_CALLBACK: Option<fn(&str, bool)> = None;

/// IPC capture state for buffering stdout/stderr during evaluate requests.
struct IpcCaptureState {
    visible: bool,
    stdout: String,
    stderr: String,
}

static IPC_CAPTURE: RwLock<IpcCaptureState> = RwLock::new(IpcCaptureState {
    visible: false,
    stdout: String::new(),
    stderr: String::new(),
});

/// Reprex mode settings.
pub(super) struct ReprexSettings {
    pub(super) enabled: bool,
    pub(super) comment: String,
    /// Buffer for partial line output (R sends output in chunks).
    pub(super) line_buffer: String,
    /// Whether output was produced since the last prompt.
    pub(super) had_output: bool,
}

pub(super) static REPREX_SETTINGS: RwLock<ReprexSettings> = RwLock::new(ReprexSettings {
    enabled: false,
    comment: String::new(),
    line_buffer: String::new(),
    had_output: false,
});

/// R's WriteConsoleEx callback.
///
/// # Safety
/// This function is called by R and must match the expected signature.
pub(super) unsafe extern "C" fn r_write_console_ex(
    buf: *const c_char,
    buflen: c_int,
    otype: c_int,
) {
    if buf.is_null() {
        return;
    }

    // Stop the spinner when R produces output
    // This provides immediate feedback that R is no longer "thinking"
    stop_spinner();

    // Check if stderr is suppressed (during completion) - only affects error output
    let is_error = otype != 0;
    if is_error && is_stderr_suppressed() {
        return;
    }

    // Debug: log raw bytes received
    let slice = unsafe { std::slice::from_raw_parts(buf as *const u8, buflen as usize) };
    log::debug!(
        "r_write_console_ex: buflen={}, otype={}, bytes={:?}",
        buflen,
        otype,
        slice
    );

    // On Windows, R sends console formatting escape sequences that are not valid UTF-8:
    // - STX (0x02) + 0xFF 0xFE = start formatting
    // - ETX (0x03) + 0xFF 0xFE = end formatting
    // We need to strip these before decoding.
    #[cfg(windows)]
    let processed: std::borrow::Cow<[u8]> = {
        if slice
            .windows(3)
            .any(|w| (w[0] == 0x02 || w[0] == 0x03) && w[1] == 0xFF && w[2] == 0xFE)
        {
            std::borrow::Cow::Owned(strip_r_format_escapes(slice))
        } else {
            std::borrow::Cow::Borrowed(slice)
        }
    };
    #[cfg(not(windows))]
    let processed: &[u8] = slice;

    // Try UTF-8 first, fall back to platform-specific encoding.
    // Note: `processed` is `Cow<[u8]>` on Windows but `&[u8]` on other platforms,
    // so we need separate cfg blocks to avoid clippy warnings.
    #[cfg(windows)]
    let s: std::borrow::Cow<str> = match std::str::from_utf8(&processed) {
        Ok(s) => std::borrow::Cow::Borrowed(s),
        Err(_) => {
            log::debug!("r_write_console_ex: UTF-8 decode failed");
            // On Windows, decode using the system's ANSI code page
            decode_windows_native(&processed)
        }
    };

    #[cfg(not(windows))]
    let s: std::borrow::Cow<str> = match std::str::from_utf8(processed) {
        Ok(s) => std::borrow::Cow::Borrowed(s),
        Err(_) => {
            log::debug!("r_write_console_ex: UTF-8 decode failed");
            // On Unix, fall back to lossy UTF-8 conversion
            String::from_utf8_lossy(processed)
        }
    };

    let is_error = otype != 0;

    // Check for custom callback first
    if let Some(callback) = unsafe { WRITE_CONSOLE_CALLBACK } {
        callback(&s, is_error);
        return;
    }

    // Check for reprex mode
    if let Ok(mut settings) = REPREX_SETTINGS.write()
        && settings.enabled
    {
        // In reprex mode, we need to handle dynamic terminal output:
        // 1. Strip ANSI escape sequences (colors, cursor movement, etc.)
        // 2. Handle carriage returns (\r) used by progress bars
        let cleaned = strip_ansi_escapes(&s);

        // Process the cleaned string character by character
        for ch in cleaned.chars() {
            match ch {
                '\n' => {
                    // Newline: print the buffered line with prefix
                    println!("{}{}", settings.comment, settings.line_buffer);
                    settings.line_buffer.clear();
                    settings.had_output = true;
                }
                '\r' => {
                    // Carriage return: clear the buffer (progress bar overwrite)
                    // This means only the final state before \n will be shown
                    settings.line_buffer.clear();
                }
                _ => {
                    settings.line_buffer.push(ch);
                }
            }
        }

        return;
    }

    // Default: print to stdout/stderr
    if is_error {
        // On Windows, R may produce CR characters in error messages which cause
        // display issues (the CR returns cursor to start of line, overwriting
        // previous content). Strip CR characters from error output only to
        // preserve progress bar functionality in normal output.
        #[cfg(windows)]
        let s = strip_cr(&s);

        // Wrap error output in red ANSI codes (like radian does)
        eprint!("{}", format_error_output(&s));
    } else {
        print!("{}", s);
        // Flush stdout immediately so progress bars using \r without \n
        // are displayed in real time instead of accumulating in the buffer.
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

/// ANSI escape code for red text.
const ANSI_RED: &str = "\x1b[31m";
/// ANSI escape code to reset text formatting.
const ANSI_RESET: &str = "\x1b[0m";

/// Format text as error output with red color.
///
/// Wraps the input string in ANSI escape codes to display it in red.
/// This matches the behavior of radian's stderr_format.
pub(super) fn format_error_output(s: &str) -> String {
    format!("{}{}{}", ANSI_RED, s, ANSI_RESET)
}

/// Strip ANSI escape sequences from a string.
///
/// ANSI escapes start with ESC (0x1B) followed by '[' and end with a letter.
/// Examples: \x1b[31m (red), \x1b[0m (reset), \x1b[2K (clear line)
pub(super) fn strip_ansi_escapes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Start of escape sequence
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // Skip until we find a letter (end of sequence)
                while let Some(&next) = chars.peek() {
                    chars.next();
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            // Also handle other escape sequences like ESC followed by single char
        } else {
            result.push(ch);
        }
    }

    result
}

/// Strip all carriage return characters from a string.
///
/// On Windows, R may produce CR (`\r`) in error messages, causing the cursor
/// to return to the start of the line and overwrite previous content.
///
/// This function removes all CR characters to prevent these issues.
/// Both CRLF (`\r\n`) and standalone CR are handled.
///
/// Returns a `Cow<str>` to avoid allocation when no CR characters are present.
#[cfg(any(windows, test))]
pub(super) fn strip_cr(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains('\r') {
        std::borrow::Cow::Owned(s.replace('\r', ""))
    } else {
        std::borrow::Cow::Borrowed(s)
    }
}

/// Strip R's Windows console formatting escape sequences.
///
/// On Windows, R sends special escape sequences for console formatting:
/// - STX (0x02) + 0xFF 0xFE = start formatting
/// - ETX (0x03) + 0xFF 0xFE = end formatting
///
/// These are not valid UTF-8 and need to be stripped before decoding.
#[cfg(windows)]
fn strip_r_format_escapes(input: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        // Check for STX/ETX + 0xFF 0xFE escape sequence
        if i + 2 < input.len()
            && (input[i] == 0x02 || input[i] == 0x03)
            && input[i + 1] == 0xFF
            && input[i + 2] == 0xFE
        {
            i += 3; // Skip the escape sequence
        } else {
            result.push(input[i]);
            i += 1;
        }
    }
    result
}

/// Decode bytes from Windows native encoding (ANSI code page) to UTF-8.
///
/// Uses the Windows `GetACP()` API to determine the system's ANSI code page,
/// then decodes using the corresponding encoding from encoding_rs.
///
/// Supported code pages:
/// - CP932 (Shift-JIS) - Japanese
/// - CP936 (GBK) - Simplified Chinese
/// - CP949 - Korean
/// - CP950 (Big5) - Traditional Chinese
/// - CP1250-1258 - Various Windows code pages
/// - And more via encoding_rs
#[cfg(windows)]
pub(super) fn decode_windows_native(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;

    // Get the system ANSI code page
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetACP() -> u32;
    }

    let code_page = unsafe { GetACP() };
    log::debug!("decode_windows_native: code_page={}", code_page);

    // Map Windows code page to encoding_rs encoding
    let encoding = match code_page {
        932 => encoding_rs::SHIFT_JIS,     // Japanese
        936 => encoding_rs::GBK,           // Simplified Chinese
        949 => encoding_rs::EUC_KR,        // Korean
        950 => encoding_rs::BIG5,          // Traditional Chinese
        874 => encoding_rs::WINDOWS_874,   // Thai
        1250 => encoding_rs::WINDOWS_1250, // Central European
        1251 => encoding_rs::WINDOWS_1251, // Cyrillic
        1252 => encoding_rs::WINDOWS_1252, // Western European
        1253 => encoding_rs::WINDOWS_1253, // Greek
        1254 => encoding_rs::WINDOWS_1254, // Turkish
        1255 => encoding_rs::WINDOWS_1255, // Hebrew
        1256 => encoding_rs::WINDOWS_1256, // Arabic
        1257 => encoding_rs::WINDOWS_1257, // Baltic
        1258 => encoding_rs::WINDOWS_1258, // Vietnamese
        65001 => encoding_rs::UTF_8,       // UTF-8 (already handled, but just in case)
        _ => {
            // Unknown code page, fall back to lossy UTF-8
            log::warn!(
                "decode_windows_native: unknown code page {}, using lossy UTF-8",
                code_page
            );
            return String::from_utf8_lossy(bytes);
        }
    };

    // Decode using the detected encoding
    let (decoded, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        log::debug!("decode_windows_native: decoding had errors");
    }

    match decoded {
        Cow::Borrowed(s) => Cow::Owned(s.to_string()),
        Cow::Owned(s) => Cow::Owned(s),
    }
}

/// Set the console write callback.
///
/// The callback receives the output string and a boolean indicating if it's an error.
pub fn set_write_console_callback(callback: fn(&str, bool)) {
    unsafe {
        WRITE_CONSOLE_CALLBACK = Some(callback);
    }
}

/// Clear the console write callback.
pub fn clear_write_console_callback() {
    unsafe {
        WRITE_CONSOLE_CALLBACK = None;
    }
}

/// WriteConsoleEx callback for IPC capture.
///
/// Buffers output into `IPC_CAPTURE`. If `visible` is set, also writes to
/// the terminal (default stdout/stderr behavior).
fn ipc_capture_callback(s: &str, is_error: bool) {
    let visible = {
        let mut state = IPC_CAPTURE.write().unwrap_or_else(|e| e.into_inner());
        if is_error {
            state.stderr.push_str(s);
        } else {
            state.stdout.push_str(s);
        }
        state.visible
    };
    // Lock is dropped before any I/O to avoid holding it during blocking writes

    if visible {
        // Also output to terminal
        if is_error {
            // On Windows, strip_cr returns Cow<str>; on Unix s is already &str
            #[cfg(not(windows))]
            eprint!("{}", format_error_output(s));
            #[cfg(windows)]
            eprint!("{}", format_error_output(&strip_cr(s)));
        } else {
            print!("{}", s);
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    }
}

/// Start IPC output capture.
///
/// Installs `ipc_capture_callback` as the WriteConsoleEx callback to buffer
/// all R console output. If `visible` is true, output is also written to the
/// terminal in real time.
///
/// Any previous capture state is reset (guards against leaked state from panics).
pub fn start_ipc_capture(visible: bool) {
    {
        let mut state = IPC_CAPTURE.write().unwrap_or_else(|e| e.into_inner());
        state.visible = visible;
        state.stdout.clear();
        state.stderr.clear();
    }
    set_write_console_callback(ipc_capture_callback);
}

/// Finish IPC output capture and return captured (stdout, stderr).
///
/// Clears the callback and returns ANSI-stripped output.
pub fn finish_ipc_capture() -> (String, String) {
    clear_write_console_callback();
    let mut state = IPC_CAPTURE.write().unwrap_or_else(|e| e.into_inner());
    let stdout = strip_ansi_escapes(&std::mem::take(&mut state.stdout));
    let stderr = strip_ansi_escapes(&std::mem::take(&mut state.stderr));
    (stdout, stderr)
}

/// Enable reprex mode with the given comment prefix.
///
/// In reprex mode, all R output is prefixed with the comment string.
/// This is useful for generating reproducible examples.
pub fn set_reprex_mode(enabled: bool, comment: &str) {
    if let Ok(mut settings) = REPREX_SETTINGS.write() {
        // If disabling reprex mode, flush any remaining buffer content
        if settings.enabled && !enabled && !settings.line_buffer.is_empty() {
            println!("{}{}", settings.comment, settings.line_buffer);
            settings.line_buffer.clear();
        }
        settings.enabled = enabled;
        settings.comment = comment.to_string();
        settings.line_buffer.clear();
        settings.had_output = false;
    }
}

/// Flush any buffered reprex output.
///
/// Call this after R evaluation to ensure partial lines are printed.
pub fn flush_reprex_buffer() {
    if let Ok(mut settings) = REPREX_SETTINGS.write()
        && settings.enabled
        && !settings.line_buffer.is_empty()
    {
        // Print remaining content with prefix and newline
        // This handles cat() output without trailing newline
        println!("{}{}", settings.comment, settings.line_buffer);
        settings.line_buffer.clear();
        settings.had_output = true;
    }
}
