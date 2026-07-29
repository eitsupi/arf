//! Platform-specific R library loading and initialization.

mod askpass;
mod discovery;
mod error_state;
mod init;
mod interrupt;
mod output;
mod spinner;

#[cfg(unix)]
pub use askpass::askpass_handler_code;
#[cfg(unix)]
pub use discovery::ensure_ld_library_path_with_pre_exec;
pub use discovery::{
    ensure_ld_library_path, find_r_library, get_r_home, r_home_from_library_path, r_library_path,
};
pub use error_state::{
    command_had_error, global_error_handler_code, mark_error_condition,
    mark_global_error_handler_initialized, reset_command_error_state, restore_stderr,
    suppress_stderr,
};
pub use init::{initialize_r, initialize_r_with_args, run_r_mainloop};
pub use interrupt::{
    clear_r_interrupt_pending, is_r_awaiting_console_input, is_r_interrupt_flag_available,
    process_r_events, set_r_interrupt_pending,
};
pub use output::{
    clear_write_console_callback, finish_ipc_capture, flush_reprex_buffer, set_reprex_mode,
    set_write_console_callback, start_ipc_capture,
};
pub use spinner::{
    is_spinner_active, set_spinner_color, set_spinner_frames, start_spinner, stop_spinner,
};

#[cfg(test)]
use discovery::{parse_var_from_wrapper_script, set_r_path_vars_from_wrapper};
#[cfg(test)]
use output::{format_error_output, strip_ansi_escapes, strip_cr};
#[cfg(test)]
use spinner::SPINNER_THREAD;

use std::os::raw::{c_char, c_int};

#[cfg(unix)]
use askpass::{ASKPASS_PROMPT_PREFIX, read_password_from_tty, recover_pending_termios};
use interrupt::AwaitConsoleInputGuard;
use output::REPREX_SETTINGS;

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
pub(super) unsafe extern "C" fn r_read_console(
    prompt: *const c_char,
    buf: *mut c_char,
    buflen: c_int,
    _hist: c_int,
) -> c_int {
    log::info!("r_read_console: called with buflen={}", buflen);

    // Stop the spinner when a new prompt is displayed
    // This handles cases where R finishes evaluation without producing output
    stop_spinner();

    // Clear any pending interrupt flag at the start of every ReadConsole
    // invocation (including nested prompts such as readline(), browser(), etc.).
    // This prevents stale Ctrl+C signals from interrupting the next input read.
    clear_r_interrupt_pending();

    // Safety net: if a previous password read (via rpassword) was interrupted
    // by longjmp (SIGINT), the terminal settings snapshot stored in
    // PENDING_TERMIOS_RESTORE may still need to be reapplied. Recover here at
    // the earliest safe point by restoring any pending termios state.
    #[cfg(unix)]
    recover_pending_termios();

    // Askpass mode: detect magic prefix, strip it, read from /dev/tty with echo disabled.
    // This runs before the input-wait guard on purpose: no Rust loop pumps R
    // events during the blocking tty read, so the longjmp hazard the guard
    // prevents does not exist here, and dropping SIGINT would make Ctrl+C at
    // a password prompt entirely inert. With the flag allowed through,
    // SA_RESTART resumes the read and the pending interrupt cancels the
    // requesting operation as soon as the read returns.
    #[cfg(unix)]
    if !prompt.is_null() {
        let prompt_bytes = unsafe { std::ffi::CStr::from_ptr(prompt) }.to_bytes();
        if prompt_bytes.starts_with(ASKPASS_PROMPT_PREFIX) {
            let real_prompt = unsafe { prompt.add(ASKPASS_PROMPT_PREFIX.len()) };
            return unsafe { read_password_from_tty(real_prompt, buf, buflen) };
        }
    }

    // Mark that R is waiting for console input until this call returns, so
    // the Ctrl+C handler drops interrupts instead of setting R's flag while
    // Rust input loops are pumping R events (see R_AWAITING_CONSOLE_INPUT).
    let _await_input_guard = AwaitConsoleInputGuard::new();

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

#[cfg(test)]
mod tests;
