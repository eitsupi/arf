//! Platform-specific R library loading and initialization.

use crate::error::{RError, RResult};
use crate::functions::{init_r_library, r_library};
use std::env;
use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::path::PathBuf;
use std::process::Command;

/// Default R library paths by platform.
#[cfg(target_os = "linux")]
const R_LIB_PATHS: &[&str] = &[
    "/opt/R/current/lib/R/lib/libR.so",
    "/usr/lib/R/lib/libR.so",
    "/usr/local/lib/R/lib/libR.so",
];

#[cfg(target_os = "macos")]
const R_LIB_PATHS: &[&str] = &[
    "/Library/Frameworks/R.framework/Versions/Current/Resources/lib/libR.dylib",
    "/opt/R/arm64/lib/R/lib/libR.dylib",
    "/usr/local/lib/R/lib/libR.dylib",
];

/// Default R library paths for Windows.
/// On Windows, R installation paths vary widely, so we rely primarily on
/// R_HOME environment variable or finding R in PATH.
#[cfg(target_os = "windows")]
const R_LIB_PATHS: &[&str] = &[];

/// Get the R shared library folder relative to R_HOME for each platform.
#[cfg(unix)]
fn r_lib_folder() -> &'static str {
    "lib"
}

#[cfg(windows)]
fn r_lib_folder() -> &'static str {
    // On Windows x64, R.dll is in bin/x64/
    // On Windows ARM64, R.dll is in bin/
    #[cfg(target_arch = "aarch64")]
    {
        "bin"
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        "bin\\x64"
    }
}

/// Find the R shared library path.
pub fn find_r_library() -> RResult<PathBuf> {
    // First, check R_HOME environment variable
    if let Ok(r_home) = env::var("R_HOME") {
        let lib_path = PathBuf::from(&r_home)
            .join(r_lib_folder())
            .join(r_lib_name());
        if lib_path.exists() {
            return Ok(lib_path);
        }
    }

    // Try to get R_HOME from R itself
    #[cfg(unix)]
    let r_cmd = "R";
    #[cfg(windows)]
    let r_cmd = "R.exe";

    if let Ok(output) = Command::new(r_cmd).args(["RHOME"]).output()
        && output.status.success()
    {
        let r_home = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let lib_path = PathBuf::from(&r_home)
            .join(r_lib_folder())
            .join(r_lib_name());
        if lib_path.exists() {
            return Ok(lib_path);
        }
    }

    // Try default paths
    for path in R_LIB_PATHS {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(path);
        }
    }

    Err(RError::LibraryNotFound(
        "Could not find R library. Please set R_HOME or ensure R is in PATH.".to_string(),
    ))
}

/// Get the R library filename for the current platform.
#[cfg(target_os = "linux")]
fn r_lib_name() -> &'static str {
    "libR.so"
}

#[cfg(target_os = "macos")]
fn r_lib_name() -> &'static str {
    "libR.dylib"
}

#[cfg(target_os = "windows")]
fn r_lib_name() -> &'static str {
    "R.dll"
}

/// Get R_HOME from the system.
pub fn get_r_home() -> RResult<PathBuf> {
    // Check environment variable first
    if let Ok(r_home) = env::var("R_HOME") {
        return Ok(PathBuf::from(r_home));
    }

    // Try to get from R command
    let output = Command::new("R")
        .args(["RHOME"])
        .output()
        .map_err(|e| RError::LibraryNotFound(format!("Failed to run R RHOME: {}", e)))?;

    if output.status.success() {
        let r_home = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(PathBuf::from(r_home))
    } else {
        Err(RError::LibraryNotFound(
            "R RHOME failed. Is R installed and in PATH?".to_string(),
        ))
    }
}

use std::sync::RwLock;

/// Console write callback function pointer storage.
static mut WRITE_CONSOLE_CALLBACK: Option<fn(&str, bool)> = None;

/// Reprex mode settings.
struct ReprexSettings {
    enabled: bool,
    comment: String,
    /// Buffer for partial line output (R sends output in chunks).
    line_buffer: String,
    /// Whether output was produced since the last prompt.
    had_output: bool,
}

static REPREX_SETTINGS: RwLock<ReprexSettings> = RwLock::new(ReprexSettings {
    enabled: false,
    comment: String::new(),
    line_buffer: String::new(),
    had_output: false,
});

/// Spinner configuration (frames and color to display).
struct SpinnerConfig {
    /// Animation frames as a string (each character is one frame).
    frames: String,
    /// ANSI color code for the spinner (e.g., "\x1b[36m" for cyan).
    color_code: String,
}

static SPINNER_CONFIG: RwLock<SpinnerConfig> = RwLock::new(SpinnerConfig {
    frames: String::new(),
    color_code: String::new(),
});

/// Spinner thread state for animated busy indicator.
/// Uses a separate thread to animate the spinner while R is evaluating code.
struct SpinnerThread {
    /// Signal to stop the spinner thread.
    stop_signal: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Handle to the spinner thread.
    handle: Option<std::thread::JoinHandle<()>>,
}

static SPINNER_THREAD: std::sync::Mutex<Option<SpinnerThread>> = std::sync::Mutex::new(None);

/// Tracks whether an error condition was signaled via globalCallingHandlers.
/// This catches rlang/dplyr errors that output to stdout instead of stderr.
static CONDITION_ERROR_OCCURRED: RwLock<bool> = RwLock::new(false);

/// Tracks whether stderr output should be suppressed.
/// When true, r_write_console_ex silently drops stderr output (otype != 0).
/// Used during completion to prevent error messages from interfering with the UI.
/// This matches radian's suppress_stderr pattern.
static SUPPRESS_STDERR: RwLock<bool> = RwLock::new(false);

/// Tracks whether the global error handler has been initialized.
/// This prevents calling R functions before the handler environment exists.
static GLOBAL_ERROR_HANDLER_INITIALIZED: RwLock<bool> = RwLock::new(false);

/// Reset the error state for the current command.
///
/// Call this before executing a new command to track errors accurately.
pub fn reset_command_error_state() {
    if let Ok(mut state) = CONDITION_ERROR_OCCURRED.write() {
        *state = false;
    }
    // Also reset the R-side error state
    reset_r_error_state();
}

/// Mark that an error condition was signaled.
///
/// This is called from the global error handler set up by `initialize_global_error_handler()`.
#[allow(dead_code)]
pub fn mark_error_condition() {
    if let Ok(mut state) = CONDITION_ERROR_OCCURRED.write() {
        *state = true;
    }
}

/// Check if the current command produced an error.
///
/// Returns `true` if either:
/// - An error condition was signaled via R's condition system (globalCallingHandlers), OR
/// - The R-side error state was set via options(error = ...) handler
///
/// Note: We rely on R's error handling mechanism rather than checking stderr output,
/// because many R functions (e.g., install.packages) write informational messages
/// to stderr that are not errors.
pub fn command_had_error() -> bool {
    let had_condition = CONDITION_ERROR_OCCURRED.read().map(|s| *s).unwrap_or(false);
    let had_r_error = check_r_error_state();
    had_condition || had_r_error
}

/// Suppress stderr output from R.
///
/// While suppressed, `r_write_console_ex` will silently drop stderr output
/// (otype != 0). Stdout output is not affected.
///
/// This is used during completion to prevent error messages from interfering
/// with the terminal display, matching radian's suppress_stderr pattern.
///
/// Use `restore_stderr()` to re-enable stderr output.
pub fn suppress_stderr() {
    if let Ok(mut state) = SUPPRESS_STDERR.write() {
        *state = true;
    }
}

/// Restore stderr output after suppression.
///
/// Call this after `suppress_stderr()` to re-enable normal stderr output.
pub fn restore_stderr() {
    if let Ok(mut state) = SUPPRESS_STDERR.write() {
        *state = false;
    }
}

/// Check if stderr output is currently suppressed.
fn is_stderr_suppressed() -> bool {
    SUPPRESS_STDERR.read().map(|s| *s).unwrap_or(false)
}

/// R's WriteConsoleEx callback.
///
/// # Safety
/// This function is called by R and must match the expected signature.
unsafe extern "C" fn r_write_console_ex(buf: *const c_char, buflen: c_int, otype: c_int) {
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
    // On Windows, R may produce CR characters which cause display issues
    // (the CR returns cursor to start of line, overwriting previous content).
    // Strip all CR characters before printing.
    #[cfg(windows)]
    let s = strip_cr(&s);

    if is_error {
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
fn format_error_output(s: &str) -> String {
    format!("{}{}{}", ANSI_RED, s, ANSI_RESET)
}

/// Strip ANSI escape sequences from a string.
///
/// ANSI escapes start with ESC (0x1B) followed by '[' and end with a letter.
/// Examples: \x1b[31m (red), \x1b[0m (reset), \x1b[2K (clear line)
fn strip_ansi_escapes(s: &str) -> String {
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
/// On Windows, R may produce CR (`\r`) characters in output messages.
/// When printed to the terminal, the CR returns the cursor to the
/// start of the line, causing subsequent text to overwrite previous content.
/// This results in garbled output.
///
/// This function removes all CR characters to prevent this issue.
/// Both CRLF (`\r\n`) and standalone CR are handled.
#[cfg(any(windows, test))]
fn strip_cr(s: &str) -> std::borrow::Cow<'_, str> {
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
fn decode_windows_native(bytes: &[u8]) -> std::borrow::Cow<'_, str> {
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

/// Check if LD_LIBRARY_PATH includes the R library directory.
/// If not, re-execute the current process with the correct LD_LIBRARY_PATH.
///
/// This is necessary because LD_LIBRARY_PATH must be set before the process
/// starts for R packages to find libR.so when loading their shared libraries.
///
/// Returns Ok(true) if re-exec happened (caller should exit),
/// Ok(false) if no re-exec needed.
#[cfg(unix)]
pub fn ensure_ld_library_path() -> RResult<bool> {
    let lib_path = find_r_library()?;
    let Some(lib_dir) = lib_path.parent() else {
        return Ok(false);
    };

    let lib_dir_str = lib_dir.to_string_lossy();
    let current = env::var("LD_LIBRARY_PATH").unwrap_or_default();

    // Check if lib_dir is already in LD_LIBRARY_PATH
    if current.split(':').any(|p| p == lib_dir_str) {
        return Ok(false);
    }

    // Need to re-exec with updated LD_LIBRARY_PATH
    let new_path = if current.is_empty() {
        lib_dir_str.to_string()
    } else {
        format!("{}:{}", lib_dir_str, current)
    };

    // SAFETY: We're about to exec, so modifying environment is safe
    unsafe { env::set_var("LD_LIBRARY_PATH", &new_path) };

    // Re-execute the current process
    let args: Vec<_> = env::args().collect();
    let exe = env::current_exe().map_err(|e| RError::LibraryNotFound(e.to_string()))?;

    log::info!("Re-executing with LD_LIBRARY_PATH={}", new_path);

    let err = exec::execvp(&exe, &args);
    Err(RError::LibraryNotFound(format!(
        "Failed to re-exec: {}",
        err
    )))
}

#[cfg(not(unix))]
pub fn ensure_ld_library_path() -> RResult<bool> {
    Ok(false)
}

/// Initialize R with default settings.
///
/// # Safety
/// This function initializes R's global state and must only be called once.
pub unsafe fn initialize_r() -> RResult<()> {
    // Use default arguments
    // Note: --interactive is only needed on Unix; Windows uses Rstart.r_interactive
    #[cfg(unix)]
    let args = &["--quiet", "--no-save", "--no-restore-data", "--interactive"];
    #[cfg(windows)]
    let args = &["--quiet", "--no-save", "--no-restore-data"];

    // SAFETY: We're forwarding to initialize_r_with_args which handles the unsafe operations
    unsafe { initialize_r_with_args(args) }
}

/// Initialize R with custom arguments.
///
/// The `r_args` parameter should contain R command-line arguments like
/// `["--quiet", "--no-save", "--no-restore"]`.
///
/// # Safety
/// This function initializes R's global state and must only be called once.
pub unsafe fn initialize_r_with_args(r_args: &[&str]) -> RResult<()> {
    // Enable color output for R packages (cli, crayon, etc.)
    // Embedded R doesn't have a TTY, so we force color output via environment variables.
    // SAFETY: We're in single-threaded initialization before R starts
    unsafe {
        // CLICOLOR_FORCE=1 forces color output even without a TTY
        if env::var("NO_COLOR").is_err() && env::var("CLICOLOR_FORCE").is_err() {
            env::set_var("CLICOLOR_FORCE", "1");
        }
        // COLORTERM indicates color support level
        if env::var("COLORTERM").is_err() {
            env::set_var("COLORTERM", "truecolor");
        }
    }

    // Find and load R library
    let lib_path = find_r_library()?;
    init_r_library(&lib_path)?;

    // Set R_HOME if not already set
    if env::var("R_HOME").is_err()
        && let Ok(r_home) = get_r_home()
    {
        // SAFETY: We're in single-threaded initialization
        unsafe { env::set_var("R_HOME", &r_home) };
    }

    // Set R_LIBS_SITE to ensure R can find base packages (including compiler for JIT)
    // SAFETY: We're in single-threaded initialization
    if env::var("R_LIBS_SITE").is_err()
        && let Ok(r_home) = get_r_home()
    {
        let lib_path = r_home.join("library");
        if lib_path.exists() {
            unsafe { env::set_var("R_LIBS_SITE", lib_path.to_string_lossy().as_ref()) };
        }
    }

    let lib = r_library()?;

    // Platform-specific initialization
    #[cfg(unix)]
    unsafe {
        initialize_r_unix(lib, r_args)?;
    }

    #[cfg(windows)]
    unsafe {
        initialize_r_windows(lib, r_args)?;
    }

    Ok(())
}

/// Unix-specific R initialization.
#[cfg(unix)]
unsafe fn initialize_r_unix(lib: &crate::functions::RLibrary, r_args: &[&str]) -> RResult<()> {
    unsafe {
        // Set R_running_as_main_program before initialization (like ark does)
        if !lib.r_running_as_main_program.is_null() {
            *lib.r_running_as_main_program = 1;
        }

        // Disable R's signal handlers before initialization
        if !lib.r_signalhandlers.is_null() {
            *lib.r_signalhandlers = 0;
        }

        // Prepare arguments for R initialization
        let mut args: Vec<CString> = vec![CString::new("arf").unwrap()];
        for arg in r_args {
            if let Ok(cstr) = CString::new(*arg) {
                args.push(cstr);
            }
        }
        let arg_ptrs: Vec<*const c_char> = args.iter().map(|s| s.as_ptr()).collect();

        // Initialize R
        (lib.rf_initialize_r)(args.len() as c_int, arg_ptrs.as_ptr());

        // Mark R as interactive
        if !lib.r_interactive.is_null() {
            *lib.r_interactive = 1;
        }

        // Disable stack checking (required for embedded R)
        if !lib.r_cstacklimit.is_null() {
            *lib.r_cstacklimit = usize::MAX;
        }

        // Redirect console output (set file pointers to NULL so callbacks are used)
        if !lib.r_consolefile.is_null() {
            *lib.r_consolefile = std::ptr::null_mut();
        }
        if !lib.r_outputfile.is_null() {
            *lib.r_outputfile = std::ptr::null_mut();
        }

        // Disable default console write
        if !lib.ptr_r_writeconsole.is_null() {
            *lib.ptr_r_writeconsole = None;
        }

        // Set our console write callback
        if !lib.ptr_r_writeconsoleex.is_null() {
            *lib.ptr_r_writeconsoleex = Some(r_write_console_ex);
        }

        // Setup R main loop (but don't run it)
        (lib.setup_rmainloop)();
    }

    Ok(())
}

/// Enable virtual terminal processing on Windows.
///
/// This is required for ANSI escape sequences (colors) to work in the Windows console.
/// Without this, escape codes like `\x1b[31m` (red) are printed literally instead
/// of being interpreted as formatting.
#[cfg(windows)]
fn enable_windows_virtual_terminal() {
    use std::os::windows::io::AsRawHandle;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetConsoleMode(handle: *mut std::ffi::c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut std::ffi::c_void, mode: u32) -> i32;
    }

    const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;

    unsafe {
        // Enable for stdout
        let stdout = std::io::stdout().as_raw_handle();
        let mut mode: u32 = 0;
        if GetConsoleMode(stdout as *mut _, &mut mode) != 0 {
            if SetConsoleMode(stdout as *mut _, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0 {
                log::debug!("[WINDOWS] Enabled virtual terminal processing for stdout");
            }
        }

        // Enable for stderr
        let stderr = std::io::stderr().as_raw_handle();
        if GetConsoleMode(stderr as *mut _, &mut mode) != 0 {
            if SetConsoleMode(stderr as *mut _, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0 {
                log::debug!("[WINDOWS] Enabled virtual terminal processing for stderr");
            }
        }
    }
}

/// Windows-specific R initialization.
///
/// On Windows, R uses a params-based approach instead of global function pointers.
/// We need to create an Rstart struct, set callbacks on it, then call R_SetParams.
/// This follows the ark pattern for Windows R initialization.
#[cfg(windows)]
unsafe fn initialize_r_windows(lib: &crate::functions::RLibrary, r_args: &[&str]) -> RResult<()> {
    use crate::types::{R_FALSE, Rstart, UImode};
    use std::mem::MaybeUninit;

    log::info!("[WINDOWS] initialize_r_windows called (ark pattern)");

    // Enable ANSI escape sequences for colored output
    enable_windows_virtual_terminal();

    // Get R_HOME and user home
    // These must be set before R_SetParams because it accesses them
    let r_home = get_r_home()?;
    let r_home_cstr = CString::new(r_home.to_string_lossy().as_ref())
        .map_err(|_| crate::error::RError::LibraryNotFound("Invalid R_HOME path".to_string()))?;

    let user_home = env::var("USERPROFILE")
        .or_else(|_| env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    let user_home_cstr = CString::new(user_home)
        .map_err(|_| crate::error::RError::LibraryNotFound("Invalid user home path".to_string()))?;

    unsafe {
        // Disable R's signal handlers first
        if !lib.r_signalhandlers.is_null() {
            *lib.r_signalhandlers = 0;
        }

        // Step 1: Call cmdlineoptions with empty args (ark pattern)
        // R does initialization here that's not accessible in any other way
        let empty_arg = CString::new("arf").unwrap();
        let mut empty_args: Vec<*mut c_char> = vec![empty_arg.as_ptr() as *mut c_char];
        (lib.cmdlineoptions)(1, empty_args.as_mut_ptr());
        log::info!("[WINDOWS] cmdlineoptions called with empty args");

        // Step 2: Create and initialize the Rstart params struct
        let mut params: MaybeUninit<Rstart> = MaybeUninit::uninit();
        let params_ptr = params.as_mut_ptr();

        // Initialize with defaults (version 0 for compatibility)
        (lib.r_defparamsex)(params_ptr, 0);

        // Step 3: Process command line arguments via R_common_command_line (ark pattern)
        // This sets params fields like R_Quiet, R_Verbose, SaveAction, RestoreAction
        let mut args: Vec<CString> = vec![CString::new("arf").unwrap()];
        for arg in r_args {
            if let Ok(cstr) = CString::new(*arg) {
                args.push(cstr);
            }
        }
        let mut arg_ptrs: Vec<*mut c_char> =
            args.iter().map(|s| s.as_ptr() as *mut c_char).collect();
        let mut argc = args.len() as c_int;
        (lib.r_common_command_line)(&mut argc, arg_ptrs.as_mut_ptr(), params_ptr);
        log::info!(
            "[WINDOWS] R_common_command_line processed {} args",
            args.len()
        );

        // Step 4: Configure the params (ark pattern)
        (*params_ptr).r_interactive = 1;
        // ark uses RGui mode (not RTerm or LinkDLL)
        (*params_ptr).character_mode = UImode::RGui;

        // Disable R's built-in profile loading during initialization.
        // We source .Rprofile manually in arf-console/src/main.rs after R is
        // fully initialized. This allows globalCallingHandlers() to work in
        // .Rprofile (used by packages like prompt).
        // See: https://github.com/posit-dev/ark/blob/ca75dbb466875c8d3cd04ad8fbf5684d59b31ba1/crates/ark/src/startup.rs
        (*params_ptr).load_init_file = R_FALSE;
        (*params_ptr).load_site_file = R_FALSE;

        // Set console callbacks (matching ark pattern)
        (*params_ptr).write_console = None;
        (*params_ptr).write_console_ex = Some(r_write_console_ex);
        (*params_ptr).read_console = Some(r_read_console);
        (*params_ptr).show_message = Some(r_show_message);
        (*params_ptr).yes_no_cancel = Some(r_yes_no_cancel);
        (*params_ptr).callback = Some(r_callback);
        (*params_ptr).busy = Some(r_busy);
        (*params_ptr).suicide = Some(r_suicide);
        log::info!(
            "[WINDOWS] Console callbacks set (read_console={:p})",
            r_read_console as *const ()
        );

        // Set paths
        (*params_ptr).rhome = r_home_cstr.as_ptr() as *mut c_char;
        (*params_ptr).home = user_home_cstr.as_ptr() as *mut c_char;

        // Step 5: Apply the params to R's globals
        (lib.r_setparams)(params_ptr);
        log::info!("[WINDOWS] R_SetParams called");

        // Disable stack checking (for testing - embedded R needs this)
        if !lib.r_cstacklimit.is_null() {
            *lib.r_cstacklimit = usize::MAX;
        }

        // Step 6: Initialize graphapp (required for Windows GUI)
        if let Some(ga_initapp) = lib.ga_initapp {
            ga_initapp(0, std::ptr::null_mut());
            log::info!("[WINDOWS] GA_initapp called");
        }

        // Read console config (required for proper console initialization)
        (lib.readconsolecfg)();
        log::info!("[WINDOWS] readconsolecfg called");

        // Step 7: Setup R main loop (but don't run it yet)
        log::info!("[WINDOWS] Calling setup_Rmainloop...");
        (lib.setup_rmainloop)();
        log::info!("[WINDOWS] setup_Rmainloop completed");
    }

    Ok(())
}

/// Windows callback for ProcessEvents (no-op).
#[cfg(windows)]
extern "C" fn r_callback() {
    // Do nothing
}

/// Windows callback for ShowMessage.
#[cfg(windows)]
extern "C" fn r_show_message(msg: *const c_char) {
    if !msg.is_null() {
        if let Ok(s) = unsafe { std::ffi::CStr::from_ptr(msg) }.to_str() {
            log::info!("[R ShowMessage] {}", s);
        }
    }
}

/// Windows callback for YesNoCancel.
/// Returns 1 for Yes, -1 for No, 0 for Cancel.
#[cfg(windows)]
extern "C" fn r_yes_no_cancel(question: *const c_char) -> c_int {
    // This is used during R's CleanUp when SA_SAVEASK is used.
    // We return -1 (No) to avoid saving.
    if !question.is_null() {
        if let Ok(s) = unsafe { std::ffi::CStr::from_ptr(question) }.to_str() {
            log::warn!("[R YesNoCancel] Ignoring question: '{}'. Returning NO.", s);
        }
    }
    -1 // NO
}

/// Windows callback for Busy indicator.
#[cfg(windows)]
extern "C" fn r_busy(_which: c_int) {
    // Do nothing - we don't have a busy indicator
}

/// Windows callback for Suicide (fatal error).
#[cfg(windows)]
extern "C" fn r_suicide(msg: *const c_char) {
    if !msg.is_null() {
        if let Ok(s) = unsafe { std::ffi::CStr::from_ptr(msg) }.to_str() {
            log::error!("[R FATAL] {}", s);
            eprintln!("R fatal error: {}", s);
        }
    }
    std::process::exit(1);
}

/// Set the console write callback.
///
/// The callback receives the output string and a boolean indicating if it's an error.
pub fn set_write_console_callback(callback: fn(&str, bool)) {
    unsafe {
        WRITE_CONSOLE_CALLBACK = Some(callback);
    }
}

/// ReadConsole callback storage.
static mut READ_CONSOLE_CALLBACK: Option<fn(&str) -> Option<String>> = None;

/// Buffer for input that exceeds R's buffer size.
/// When input is longer than buflen, the remainder is stored here
/// and returned on subsequent ReadConsole calls.
///
/// Note: This is accessed only from R's main thread in the ReadConsole callback.
static PENDING_INPUT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// R's ReadConsole callback.
///
/// # Safety
/// This function is called by R and must match the expected signature.
unsafe extern "C" fn r_read_console(
    prompt: *const c_char,
    buf: *mut c_char,
    buflen: c_int,
    _hist: c_int,
) -> c_int {
    log::info!("r_read_console: called with buflen={}", buflen);

    // Stop the spinner when a new prompt is displayed
    // This handles cases where R finishes evaluation without producing output
    stop_spinner();

    // In reprex mode, print a blank line between expressions for readability
    // Only print for main prompts (not continuation prompts like "+")
    if let Ok(mut settings) = REPREX_SETTINGS.write()
        && settings.enabled
        && settings.had_output
    {
        // Check if this is a main prompt (not continuation)
        let is_main_prompt = if prompt.is_null() {
            true
        } else {
            // SAFETY: prompt is a valid C string from R
            let prompt_str = unsafe { std::ffi::CStr::from_ptr(prompt) }.to_string_lossy();
            // Continuation prompts typically start with "+" or spaces
            !prompt_str.starts_with('+') && !prompt_str.trim().is_empty()
        };

        if is_main_prompt {
            println!();
            settings.had_output = false;
        }
    }

    // Get input - either from pending buffer or from callback
    let input = {
        let mut pending = PENDING_INPUT.lock().unwrap();
        if !pending.is_empty() {
            // Use pending input from previous call
            log::debug!("r_read_console: using pending input");
            std::mem::take(&mut *pending)
        } else {
            drop(pending); // Release lock before callback

            // Get the prompt string
            let prompt_str: &str = if prompt.is_null() {
                ""
            } else {
                // SAFETY: prompt is a valid C string from R
                unsafe { std::ffi::CStr::from_ptr(prompt) }
                    .to_str()
                    .unwrap_or_default()
            };

            log::debug!("r_read_console: prompt={:?}", prompt_str);

            // Call the callback to get new input
            // SAFETY: READ_CONSOLE_CALLBACK is only accessed from this single-threaded context
            if let Some(callback) = unsafe { READ_CONSOLE_CALLBACK } {
                log::debug!("r_read_console: calling callback");
                match callback(prompt_str) {
                    Some(s) => {
                        log::debug!("r_read_console: got input len={}", s.len());
                        s
                    }
                    None => {
                        log::debug!("r_read_console: callback returned None (EOF)");
                        return 0; // EOF
                    }
                }
            } else {
                log::debug!("r_read_console: no callback set, returning 0");
                return 0; // No callback set
            }
        }
    };

    let bytes = input.as_bytes();
    // Reserve 2 bytes: one for potential newline, one for null terminator
    let max_len = (buflen as usize).saturating_sub(2);

    // Find copy length, ensuring we don't split multibyte characters
    let copy_len = if bytes.len() <= max_len {
        bytes.len()
    } else {
        // Find the last valid UTF-8 boundary at or before max_len
        let mut end = max_len;
        while end > 0 && !input.is_char_boundary(end) {
            end -= 1;
        }
        end
    };

    // SAFETY: buf is a valid buffer of at least buflen bytes from R
    unsafe {
        if copy_len > 0 {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, copy_len);
        }

        let mut pos = copy_len;

        // Store remaining input for next call, or add newline if done
        if copy_len < bytes.len() {
            // More input remaining - store it for next ReadConsole call
            let mut pending = PENDING_INPUT.lock().unwrap();
            *pending = input[copy_len..].to_string();
            // No newline - R will call us again
        } else {
            // All input consumed - add newline if not present
            if bytes.is_empty() || bytes[bytes.len() - 1] != b'\n' {
                *buf.add(pos) = b'\n' as c_char;
                pos += 1;
            }
        }

        // Null terminate
        *buf.add(pos) = 0;
    }

    1
}

/// Set the console read callback.
///
/// The callback receives the prompt and should return the user's input,
/// or None to signal EOF (exit R).
pub fn set_read_console_callback(callback: fn(&str) -> Option<String>) {
    unsafe {
        READ_CONSOLE_CALLBACK = Some(callback);
    }
}

/// Run R's main event loop.
///
/// This calls R's `run_Rmainloop()` which never returns normally.
/// It will continuously call the ReadConsole callback to get user input.
///
/// # Safety
/// R must be initialized before calling this function.
pub unsafe fn run_r_mainloop() {
    log::info!("run_r_mainloop: entering");

    let lib = match r_library() {
        Ok(lib) => lib,
        Err(e) => {
            log::error!("run_r_mainloop: failed to get r_library: {:?}", e);
            return;
        }
    };

    // Set up our ReadConsole callback (Unix only - Windows sets it in R_SetParams)
    #[cfg(unix)]
    unsafe {
        if !lib.ptr_r_readconsole.is_null() {
            *lib.ptr_r_readconsole = Some(r_read_console);
        }
    }

    // Check R_Interactive value before running mainloop
    unsafe {
        if !lib.r_interactive.is_null() {
            log::info!("run_r_mainloop: R_Interactive = {}", *lib.r_interactive);
        }
    }

    log::info!("run_r_mainloop: calling run_Rmainloop");

    // Run R's main loop - this doesn't return
    unsafe {
        (lib.run_rmainloop)();
    }

    log::info!("run_r_mainloop: run_Rmainloop returned (unexpected)");
}

/// Get the R code for setting up the global error handler.
///
/// This should be evaluated after R is initialized but before the main loop starts.
/// It sets up `globalCallingHandlers()` (R >= 4.0) to track error conditions
/// that may output to stdout instead of stderr.
///
/// Call this from the application layer (e.g., arf-console) and use arf-harp's
/// eval_string to evaluate the returned code.
pub fn global_error_handler_code() -> &'static str {
    GLOBAL_ERROR_HANDLER_CODE
}

/// Mark the global error handler as initialized.
///
/// Call this after successfully evaluating `global_error_handler_code()`.
/// This enables R-side error state checking in `command_had_error()`.
pub fn mark_global_error_handler_initialized() {
    if let Ok(mut state) = GLOBAL_ERROR_HANDLER_INITIALIZED.write() {
        *state = true;
    }
}

/// Check if the global error handler has been initialized.
fn is_global_error_handler_initialized() -> bool {
    GLOBAL_ERROR_HANDLER_INITIALIZED
        .read()
        .map(|s| *s)
        .unwrap_or(false)
}

/// R code to set up the global error handler.
///
/// This uses `options(error = ...)` to intercept all errors after they occur.
/// The error handler is called at the end of R's error handling, right before
/// returning to the prompt. This catches all errors, including rlang/dplyr errors.
///
/// Note: globalCallingHandlers() doesn't work reliably in embedded R because
/// errors caught by R_ToplevelExec or similar mechanisms bypass the condition system.
///
/// The handler stores the error state in an environment variable that we can
/// check from Rust using Rf_findVar.
const GLOBAL_ERROR_HANDLER_CODE: &str = r#"
local({
    # Create an environment to store error state
    .arf_error_state <- new.env(parent = emptyenv())
    .arf_error_state$had_error <- FALSE

    # Store it in global environment for persistence
    assign(".arf_error_state", .arf_error_state, envir = globalenv())

    # Store the user's previous error handler (if any) so we can chain to it
    prev_handler <- getOption("error")
    assign(".arf_prev_error_handler", prev_handler, envir = globalenv())

    # Set up our error handler using options(error = ...)
    # This is called at the END of R's error handling, just before returning to prompt
    options(error = quote({
        # Mark that an error occurred
        env <- get(".arf_error_state", envir = globalenv())
        env$had_error <- TRUE

        # Chain to the previous handler if it exists
        prev <- get(".arf_prev_error_handler", envir = globalenv())
        if (!is.null(prev)) {
            eval(prev)
        }
    }))

    invisible(NULL)
})
"#;

/// Check if the R error state indicates an error occurred.
///
/// This reads `.arf_error_state$had_error` from the global environment.
/// The globalCallingHandlers error handler sets this to TRUE when an error occurs.
///
/// # Safety
/// R must be initialized and the global error handler must be set up
/// before this function returns meaningful results.
fn check_r_error_state() -> bool {
    // Don't check R state if the handler hasn't been initialized yet
    if !is_global_error_handler_initialized() {
        return false;
    }

    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return false,
    };

    unsafe {
        // Look up .arf_error_state in global environment using Rf_findVar
        let arf_error_state_sym = {
            let name = std::ffi::CString::new(".arf_error_state").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        let global_env = *lib.r_globalenv;
        let state_env = (lib.rf_findvar)(arf_error_state_sym, global_env);

        // Check if the environment exists
        if state_env.is_null() || state_env == *lib.r_unboundvalue {
            return false;
        }

        // Look up had_error in the state environment
        let had_error_sym = {
            let name = std::ffi::CString::new("had_error").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        let had_error = (lib.rf_findvar)(had_error_sym, state_env);

        if had_error.is_null() || had_error == *lib.r_unboundvalue {
            return false;
        }

        // Check if it's TRUE (logical vector with value != 0)
        let logical_ptr = (lib.logical)(had_error);
        if !logical_ptr.is_null() {
            return *logical_ptr != 0;
        }

        false
    }
}

/// Reset the R error state.
///
/// This should be called before each command to reset the error tracking.
/// Sets `.arf_error_state$had_error` to FALSE.
///
/// # Safety
/// R must be initialized and the global error handler must be set up
/// before this function has any effect.
fn reset_r_error_state() {
    // Don't try to reset R state if the handler hasn't been initialized yet
    if !is_global_error_handler_initialized() {
        return;
    }

    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };

    unsafe {
        // Look up .arf_error_state in global environment
        let arf_error_state_sym = {
            let name = std::ffi::CString::new(".arf_error_state").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        let global_env = *lib.r_globalenv;
        let state_env = (lib.rf_findvar)(arf_error_state_sym, global_env);

        // If the environment doesn't exist, nothing to reset
        if state_env.is_null() || state_env == *lib.r_unboundvalue {
            log::trace!("reset_r_error_state: .arf_error_state not found");
            return;
        }

        // Set had_error to FALSE using Rf_defineVar
        let had_error_sym = {
            let name = std::ffi::CString::new("had_error").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        // Create FALSE value (0)
        let false_val = (lib.rf_scalarlogical)(0);

        // Set had_error = FALSE in the state environment
        (lib.rf_definevar)(had_error_sym, false_val, state_env);
        log::trace!("reset_r_error_state: set had_error = FALSE");
    }
}

/// Configure the spinner animation frames.
///
/// The `frames` string contains characters to cycle through.
/// An empty string disables the spinner.
///
/// Example: `"⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"` for braille dots spinner.
pub fn set_spinner_frames(frames: &str) {
    if let Ok(mut config) = SPINNER_CONFIG.write() {
        config.frames = frames.to_string();
    }
}

/// Configure the spinner color.
///
/// The `color_code` should be an ANSI escape sequence for the color,
/// e.g., "\x1b[36m" for cyan.
pub fn set_spinner_color(color_code: &str) {
    if let Ok(mut config) = SPINNER_CONFIG.write() {
        config.color_code = color_code.to_string();
    }
}

/// Start the spinner (display the busy indicator).
///
/// Spawns a background thread that animates the spinner at ~12.5fps.
/// The spinner is stopped automatically when R output is produced or
/// when the next ReadConsole prompt is displayed.
pub fn start_spinner() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::Duration;

    // Get the frames and color from config
    let (frames, color_code) = match SPINNER_CONFIG.read() {
        Ok(config) => (config.frames.clone(), config.color_code.clone()),
        Err(_) => return,
    };

    if frames.is_empty() {
        return; // Spinner disabled
    }

    // Check if already running
    let mut spinner_guard = match SPINNER_THREAD.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };

    if spinner_guard.is_some() {
        return; // Already running
    }

    // Create stop signal
    let stop_signal = Arc::new(AtomicBool::new(false));
    let stop_signal_clone = stop_signal.clone();

    // ANSI reset code
    const ANSI_RESET: &str = "\x1b[0m";

    // Spawn the spinner thread
    let handle = thread::spawn(move || {
        let frames_chars: Vec<char> = frames.chars().collect();
        if frames_chars.is_empty() {
            return;
        }

        let mut frame_index = 0;
        let frame_duration = Duration::from_millis(80); // ~12.5 fps for smooth animation

        // Display the first frame with color
        if color_code.is_empty() {
            print!("{} ", frames_chars[frame_index]);
        } else {
            print!("{}{}{} ", color_code, frames_chars[frame_index], ANSI_RESET);
        }
        let _ = std::io::Write::flush(&mut std::io::stdout());

        loop {
            // Check stop signal at loop start
            if stop_signal_clone.load(Ordering::Relaxed) {
                break;
            }

            thread::sleep(frame_duration);

            // Check again after sleep for faster response to stop signal
            // This avoids unnecessary frame display when stop was called during sleep
            if stop_signal_clone.load(Ordering::Relaxed) {
                break;
            }

            // Advance to next frame
            frame_index = (frame_index + 1) % frames_chars.len();

            // Update the display: move cursor back and print new frame with color
            // \r moves to start of line, then print frame + space
            if color_code.is_empty() {
                print!("\r{} ", frames_chars[frame_index]);
            } else {
                print!(
                    "\r{}{}{} ",
                    color_code, frames_chars[frame_index], ANSI_RESET
                );
            }
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    });

    *spinner_guard = Some(SpinnerThread {
        stop_signal,
        handle: Some(handle),
    });
}

/// Stop the spinner and clear it from the display.
///
/// This is called automatically when R produces output or when
/// the next prompt is about to be displayed.
pub fn stop_spinner() {
    use std::sync::atomic::Ordering;

    let mut spinner_guard = match SPINNER_THREAD.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };

    if let Some(spinner) = spinner_guard.take() {
        // Signal the thread to stop
        spinner.stop_signal.store(true, Ordering::Relaxed);

        // Wait for the thread to finish
        if let Some(handle) = spinner.handle {
            let _ = handle.join();
        }

        // Clear the spinner from the display
        print!("\r\x1b[K");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

/// Check if the spinner is currently active.
pub fn is_spinner_active() -> bool {
    SPINNER_THREAD
        .lock()
        .map(|guard| guard.is_some())
        .unwrap_or(false)
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

/// Process R events.
///
/// This calls R's event processing functions to handle:
/// - Graphics window events (X11, Windows GDI, etc.)
/// - User interrupts
/// - Other system events
///
/// On Unix, this also runs input handlers for background tasks.
///
/// This function should be called periodically while waiting for user input
/// to keep R's interactive windows responsive.
///
/// # Current Limitations
///
/// Currently, this function is only called once before `read_line()` due to
/// reedline's blocking design. This means graphics windows are only updated
/// when the user presses a key. For fully responsive graphics windows,
/// reedline would need an idle callback feature to call this function
/// periodically during input waiting.
///
/// TODO: Consider forking reedline to add an idle callback feature, or
/// contribute such a feature upstream. See:
/// - Similar discussion: <https://github.com/nushell/reedline/issues/311>
/// - External event queue PR (closed): <https://github.com/nushell/reedline/pull/822>
///
/// # Safety
/// R must be initialized before calling this function.
pub fn process_r_events() {
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };

    unsafe {
        // Call R_ProcessEvents - this is the main event processing function
        (lib.r_processevents)();

        // Platform-specific additional event processing
        #[cfg(unix)]
        {
            // On Unix, also check for and run input handlers
            // This handles things like httpuv background requests
            if !lib.r_inputhandlers.is_null() {
                let what = (lib.r_checkactivity)(0, 1);
                if !what.is_null() {
                    (lib.r_runhandlers)(*lib.r_inputhandlers, what);
                }
            }
        }
    }
}

/// Check if there are pending R events that need processing.
///
/// This is useful for polling before calling `process_r_events()` to avoid
/// unnecessary processing when idle.
///
/// Returns `true` if there are events to process, `false` otherwise.
///
/// # Safety
/// R must be initialized before calling this function.
pub fn peek_r_event() -> bool {
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return false,
    };

    unsafe {
        #[cfg(windows)]
        {
            // On Windows, use GA_peekevent from Rgraphapp.dll
            if let Some(ga_peekevent) = lib.ga_peekevent {
                return ga_peekevent() != 0;
            }
            false
        }

        #[cfg(unix)]
        {
            // On Unix, use R_checkActivity
            if lib.r_inputhandlers.is_null() {
                return false;
            }
            let what = (lib.r_checkactivity)(0, 1);
            !what.is_null()
        }
    }
}

/// Process R events in a loop suitable for use during input waiting.
///
/// This function processes any pending events and returns. It's designed
/// to be called from an input hook or polling loop.
///
/// The pattern for use is:
/// ```ignore
/// loop {
///     if input_ready() {
///         break;
///     }
///     polled_events_for_repl();
///     std::thread::sleep(Duration::from_millis(33)); // ~30fps
/// }
/// ```
///
/// # Safety
/// R must be initialized before calling this function.
pub fn polled_events_for_repl() {
    // Check if there are pending events first
    if peek_r_event() {
        process_r_events();
    } else {
        // Even if no events are pending, call R_ProcessEvents occasionally
        // to handle R's internal housekeeping
        process_r_events();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Combined spinner test to avoid race conditions from parallel tests sharing global state.
    /// Tests spinner lifecycle: config, start, stop, double-start, double-stop, and color.
    #[test]
    fn test_spinner_lifecycle() {
        // Verify the spinner lock isn't poisoned from a previous panic.
        // We release immediately since start_spinner/stop_spinner need to acquire it.
        drop(SPINNER_THREAD.lock().unwrap());

        // Reset to known state first
        stop_spinner();
        set_spinner_frames("");

        // Test 1: Initial state should be inactive
        assert!(!is_spinner_active());

        // Test 2: Spinner disabled with empty frames
        set_spinner_frames("");
        start_spinner();
        assert!(!is_spinner_active()); // Should not be active when frames are empty

        // Test 3: Basic start/stop
        set_spinner_frames("⠋⠙⠹");
        start_spinner();
        assert!(is_spinner_active());
        stop_spinner();
        assert!(!is_spinner_active());

        // Test 4: With color
        set_spinner_frames("⠋⠙⠹");
        set_spinner_color("\x1b[36m"); // Cyan
        start_spinner();
        assert!(is_spinner_active());
        stop_spinner();
        assert!(!is_spinner_active());

        // Test 5: Double start (should be no-op)
        set_spinner_frames("⠋⠙⠹");
        start_spinner();
        assert!(is_spinner_active());
        start_spinner(); // Second start should be a no-op
        assert!(is_spinner_active());
        stop_spinner();
        assert!(!is_spinner_active());

        // Test 6: Double stop (should be no-op)
        set_spinner_frames("⠋⠙⠹");
        start_spinner();
        stop_spinner();
        assert!(!is_spinner_active());
        stop_spinner(); // Second stop should be a no-op
        assert!(!is_spinner_active());

        // Cleanup
        set_spinner_frames("");
        set_spinner_color("");
    }

    /// Test command error state tracking.
    ///
    /// These assertions are combined into a single test to avoid race conditions
    /// when tests run in parallel (they share global state via CONDITION_ERROR_OCCURRED).
    #[test]
    fn test_command_error_state() {
        // Reset to known state first
        reset_command_error_state();

        // Initially no error
        assert!(!command_had_error(), "initial state should be false");

        // Mark an error condition
        mark_error_condition();
        assert!(command_had_error(), "should detect error after mark");

        // Reset should clear the error state
        reset_command_error_state();
        assert!(
            !command_had_error(),
            "should be false after reset_command_error_state"
        );

        // Mark error again and verify detection
        mark_error_condition();
        assert!(command_had_error(), "should detect error condition");

        // Final reset
        reset_command_error_state();
        assert!(!command_had_error(), "should be false after final reset");
    }

    #[test]
    fn test_format_error_output() {
        // Basic error message
        let formatted = format_error_output("Error: foo");
        assert_eq!(formatted, "\x1b[31mError: foo\x1b[0m");

        // Empty string
        let formatted = format_error_output("");
        assert_eq!(formatted, "\x1b[31m\x1b[0m");

        // Multiline error
        let formatted = format_error_output("Error in x:\n  undefined");
        assert_eq!(formatted, "\x1b[31mError in x:\n  undefined\x1b[0m");
    }

    #[test]
    fn test_strip_ansi_escapes() {
        // Strip red color codes
        let stripped = strip_ansi_escapes("\x1b[31mError: foo\x1b[0m");
        assert_eq!(stripped, "Error: foo");

        // Strip multiple color codes
        let stripped = strip_ansi_escapes("\x1b[1m\x1b[31mBold Red\x1b[0m");
        assert_eq!(stripped, "Bold Red");

        // No escape codes
        let stripped = strip_ansi_escapes("plain text");
        assert_eq!(stripped, "plain text");

        // Complex sequence (cursor movement)
        let stripped = strip_ansi_escapes("before\x1b[2Kafter");
        assert_eq!(stripped, "beforeafter");
    }

    #[test]
    fn test_error_format_strip_roundtrip() {
        // Formatting and then stripping should give back original text
        let original = "Error: something went wrong";
        let formatted = format_error_output(original);
        let stripped = strip_ansi_escapes(&formatted);
        assert_eq!(stripped, original);
    }

    #[test]
    fn test_strip_cr() {
        // CRLF should be converted to LF
        let stripped = strip_cr("Error: foo\r\nbar\r\n");
        assert_eq!(stripped, "Error: foo\nbar\n");

        // Text without CR should be unchanged (and borrowed, not owned)
        let input = "Error: foo\nbar\n";
        let stripped = strip_cr(input);
        assert_eq!(stripped, input);
        assert!(matches!(stripped, std::borrow::Cow::Borrowed(_)));

        // Standalone CR should also be stripped
        let stripped = strip_cr("Error: \"{\r\" の)");
        assert_eq!(stripped, "Error: \"{\" の)");

        // Mixed line endings: all CR should be removed
        let stripped = strip_cr("line1\r\nline2\nline3\r");
        assert_eq!(stripped, "line1\nline2\nline3");

        // Empty string
        let stripped = strip_cr("");
        assert_eq!(stripped, "");
        assert!(matches!(stripped, std::borrow::Cow::Borrowed(_)));
    }
}
