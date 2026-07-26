#[cfg(unix)]
use std::os::raw::{c_char, c_int};

/// Magic prefix prepended to askpass prompts by the R handler.
/// SOH (\x01) and STX (\x02) are control characters that won't appear in real prompts.
/// The R handler calls `readline(paste0("\001ASKPASS\002", msg))` and the Rust
/// ReadConsole callback detects this prefix to enter password-reading mode.
#[cfg(unix)]
pub(super) const ASKPASS_PROMPT_PREFIX: &[u8] = b"\x01ASKPASS\x02";

/// Safety net for termios restoration after longjmp.
///
/// When R's SIGINT handler fires during password reading, it may longjmp
/// out of `read_password_from_tty`, bypassing the normal echo-restoration
/// path in the password-reading code. This can leave the terminal with echo
/// disabled — a critical UX issue.
///
/// Before disabling echo we store `(fd, old_termios)` here; on successful
/// restoration we clear it. `r_read_console` checks this on every entry
/// and recovers if a stale entry is found.
#[cfg(unix)]
static PENDING_TERMIOS_RESTORE: std::sync::Mutex<Option<(c_int, libc::termios)>> =
    std::sync::Mutex::new(None);

/// Recover terminal settings if a previous `read_password_from_tty` was
/// interrupted by longjmp (e.g. SIGINT → R's error handler).
///
/// Called at the top of `r_read_console` so recovery happens at the earliest
/// safe point after the longjmp lands.
#[cfg(unix)]
pub(super) fn recover_pending_termios() {
    let mut guard = match PENDING_TERMIOS_RESTORE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => {
            log::error!("askpass: PENDING_TERMIOS_RESTORE mutex poisoned, attempting recovery");
            poisoned.into_inner()
        }
    };
    if let Some((fd, old_termios)) = guard.as_ref() {
        log::warn!(
            "askpass: recovering terminal settings after interrupted password read (fd={})",
            fd
        );
        // Whether to clear the snapshot and close the fd after the loop.
        // `close_fd`: true = close fd after clearing; false = clear only (fd may be invalid).
        let mut clear_snapshot: Option<bool> = None;
        loop {
            let rc = unsafe { libc::tcsetattr(*fd, libc::TCSANOW, old_termios) };
            if rc == 0 {
                clear_snapshot = Some(true);
                break;
            }
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            log::error!("askpass: failed to recover terminal settings: {}", err);
            // Fatal errors where retry on the next r_read_console call won't help.
            // EBADF means the fd is already invalid, so skip close.
            // ENOTTY/EIO: the fd may still be valid, so close it to avoid leak.
            match err.raw_os_error() {
                Some(libc::EBADF) => clear_snapshot = Some(false),
                Some(libc::ENOTTY | libc::EIO) => clear_snapshot = Some(true),
                _ => {} // Transient — keep snapshot for retry on next call
            }
            break;
        }
        if let Some(close_fd) = clear_snapshot {
            let leaked = guard.take();
            if close_fd && let Some((fd, _)) = leaked {
                // Close the /dev/tty fd that was leaked by the interrupted read.
                // This fd is still open because longjmp skipped File's destructor;
                // the OS won't reuse an open fd number, so double-close cannot happen.
                unsafe { libc::close(fd) };
            }
        }
    }
}

/// Write an empty password ("\n\0") to R's readline buffer.
///
/// The R askpass handler treats `identical(password, "")` as cancellation
/// (returns NULL). `buf` must be non-null and `buflen` must be >= 2.
#[cfg(unix)]
unsafe fn write_empty_password(buf: *mut c_char, buflen: c_int) {
    debug_assert!(!buf.is_null() && buflen >= 2);
    unsafe {
        // The `if` guard is intentional: debug_assert is stripped in release
        // builds, so this ensures no UB even if the precondition is violated.
        if buflen >= 2 {
            *buf = b'\n' as c_char;
            *buf.add(1) = 0;
        }
    }
}

/// Read a password from `/dev/tty` with echo disabled using `rpassword`.
///
/// Called from `r_read_console` when the prompt contains [`ASKPASS_PROMPT_PREFIX`].
/// Uses `rpassword::prompt_password` which handles `/dev/tty` open, echo
/// suppression via termios, reading, restoration, and newline echo internally.
///
/// **longjmp safety**: Before calling rpassword, we snapshot the current
/// terminal state into [`PENDING_TERMIOS_RESTORE`]. If R's SIGINT handler
/// longjmps past rpassword's internal cleanup, `r_read_console` recovers
/// on its next invocation.
///
/// **Fail-closed**: on any error, writes an empty password to `buf` so R's
/// handler treats it as cancellation. Never falls back to reedline.
///
/// **Note**: `rpassword` returns a `String`, so non-UTF-8 passwords are not
/// supported. In practice passwords are virtually always ASCII/UTF-8.
///
/// # Safety
/// `buf` must be non-null and `buflen` must be >= 2. `prompt` may be null.
#[cfg(unix)]
pub(super) unsafe fn read_password_from_tty(
    prompt: *const c_char,
    buf: *mut c_char,
    buflen: c_int,
) -> c_int {
    // Runtime guard: R guarantees a valid buffer, but fail closed on FFI violation.
    if buf.is_null() || buflen < 2 {
        log::error!(
            "askpass: invalid buffer from R (null={}, buflen={})",
            buf.is_null(),
            buflen
        );
        // Fail closed: write a NUL-terminated empty string if we have any space,
        // so R never reads stale/undefined buffer contents.
        if !buf.is_null() && buflen > 0 {
            unsafe { *buf = 0 };
        }
        return 1;
    }

    let prompt_str = if prompt.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(prompt) }
            .to_string_lossy()
            .into_owned()
    };

    // Snapshot terminal state BEFORE rpassword modifies it.
    // If R's SIGINT handler longjmps during the read, rpassword's internal
    // RAII is also skipped. r_read_console checks PENDING_TERMIOS_RESTORE
    // on each entry and restores the terminal at the earliest safe point.
    //
    // Note: this fd is separate from the one rpassword opens internally.
    // On longjmp, rpassword's fd leaks (one fd table entry until process
    // exit) but the terminal state is correctly recovered via our snapshot.
    let tty_fd = unsafe { libc::open(c"/dev/tty".as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
    if tty_fd >= 0 {
        let mut old_termios: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(tty_fd, &mut old_termios) } == 0 {
            match PENDING_TERMIOS_RESTORE.lock() {
                Ok(mut pending) => *pending = Some((tty_fd, old_termios)),
                Err(_) => {
                    unsafe { libc::close(tty_fd) };
                }
            }
        } else {
            unsafe { libc::close(tty_fd) };
        }
    } else {
        log::warn!("askpass: could not open /dev/tty for termios snapshot");
    }

    let result = rpassword::prompt_password(&prompt_str);

    // Clear safety net — rpassword returned normally, terminal is restored.
    let mut pending = match PENDING_TERMIOS_RESTORE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some((fd, _)) = pending.take() {
        unsafe { libc::close(fd) };
    }

    match result {
        Ok(mut password) => {
            let password_bytes = password.as_bytes();
            let max_len = (buflen as usize).saturating_sub(2); // room for '\n' + '\0'
            if password_bytes.len() > max_len {
                log::error!(
                    "askpass: password length ({}) exceeds buffer capacity ({}), rejecting",
                    password_bytes.len(),
                    max_len
                );
                // Zeroize before drop to minimize sensitive data in memory.
                // SAFETY: we zero the backing allocation, then drop the String.
                unsafe {
                    for byte in password.as_bytes_mut() {
                        std::ptr::write_volatile(byte, 0);
                    }
                }
                unsafe { write_empty_password(buf, buflen) };
                return 1;
            }
            let copy_len = password_bytes.len();
            unsafe {
                if copy_len > 0 {
                    std::ptr::copy_nonoverlapping(
                        password_bytes.as_ptr(),
                        buf as *mut u8,
                        copy_len,
                    );
                }
                *buf.add(copy_len) = b'\n' as c_char;
                *buf.add(copy_len + 1) = 0;
            }
            // Zeroize before drop to minimize sensitive data in memory.
            // SAFETY: we zero the backing allocation, then drop the String.
            unsafe {
                for byte in password.as_bytes_mut() {
                    std::ptr::write_volatile(byte, 0);
                }
            }
            1
        }
        Err(e) => {
            log::error!("askpass: failed to read password: {}", e);
            unsafe { write_empty_password(buf, buflen) };
            1
        }
    }
}

/// R code for setting up the askpass handler. Evaluate after R init.
///
/// Sets `options(askpass = ...)` with a function that calls `readline()` with
/// [`ASKPASS_PROMPT_PREFIX`] prepended to the prompt. The Rust ReadConsole
/// callback detects the prefix and reads from `/dev/tty` with echo disabled.
///
/// `readline()` is used (rather than reading `/dev/tty` directly from R) because
/// it routes through R's ReadConsole callback, which stops the spinner.
///
/// Unix-only. On Windows, askpass uses GUI dialogs by default.
/// Does not override if the user has already set `options(askpass = ...)`.
#[cfg(unix)]
pub fn askpass_handler_code() -> &'static str {
    ASKPASS_HANDLER_CODE
}
#[cfg(unix)]
const ASKPASS_HANDLER_CODE: &str = r#"
local({
    # Only override on Unix. Windows uses GUI dialogs (win-askpass.exe) by default.
    if (.Platform$OS.type != "unix") {
        invisible(NULL)
    } else if (!is.null(getOption("askpass"))) {
        invisible(NULL)
    } else {
        options(askpass = function(msg) {
            # Prefix triggers Rust ReadConsole to read from /dev/tty with echo disabled
            password <- readline(paste0("\001ASKPASS\002", msg))
            if (identical(password, "")) return(NULL)
            password
        })
        invisible(NULL)
    }
})
"#;
