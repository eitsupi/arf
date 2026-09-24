use std::sync::RwLock;

/// Tracks whether stderr output should be suppressed.
///
/// When true, `r_write_console_ex` silently drops stderr output (`otype != 0`).
/// This is used during completion to prevent error messages from interfering
/// with the UI. Stdout output is not affected.
static SUPPRESS_STDERR: RwLock<bool> = RwLock::new(false);

/// Suppress stderr output from R until [`restore_stderr`] is called.
pub fn suppress_stderr() {
    if let Ok(mut state) = SUPPRESS_STDERR.write() {
        *state = true;
    }
}

/// Restore stderr output after suppression.
pub fn restore_stderr() {
    if let Ok(mut state) = SUPPRESS_STDERR.write() {
        *state = false;
    }
}

/// Check if stderr output is currently suppressed.
pub(super) fn is_stderr_suppressed() -> bool {
    SUPPRESS_STDERR.read().map(|state| *state).unwrap_or(false)
}
