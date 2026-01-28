//! Signal trap handlers for graceful crash handling.
//!
//! This module provides handlers for fatal signals (SIGSEGV, SIGILL, SIGBUS)
//! that log a backtrace before terminating. This prevents the process from
//! hanging when R encounters a segmentation fault.

/// Register signal handlers for fatal signals.
///
/// This installs handlers for SIGSEGV, SIGILL, and SIGBUS (Unix only)
/// that capture a backtrace and exit cleanly instead of hanging.
///
/// Call this after initializing the logger.
pub fn register_trap_handlers() {
    #[cfg(unix)]
    register_unix_handlers();

    #[cfg(windows)]
    register_windows_handlers();
}

#[cfg(unix)]
fn register_unix_handlers() {
    unsafe {
        libc::signal(libc::SIGSEGV, backtrace_handler as libc::sighandler_t);
        libc::signal(libc::SIGILL, backtrace_handler as libc::sighandler_t);
        libc::signal(libc::SIGBUS, backtrace_handler as libc::sighandler_t);
    }
}

#[cfg(windows)]
fn register_windows_handlers() {
    unsafe {
        libc::signal(libc::SIGSEGV, backtrace_handler as libc::sighandler_t);
        libc::signal(libc::SIGILL, backtrace_handler as libc::sighandler_t);
    }
}

/// Signal handler that logs a backtrace and exits.
///
/// When a fatal signal is received:
/// 1. Resets the handler to default (prevents infinite loops)
/// 2. Logs the signal and backtrace
/// 3. Lets the default handler terminate the process
extern "C-unwind" fn backtrace_handler(signum: libc::c_int) {
    // Prevent infinite loop by resetting to default handler
    unsafe {
        libc::signal(signum, libc::SIG_DFL);
    }

    let signal_name = match signum {
        libc::SIGSEGV => "SIGSEGV (segmentation fault)",
        libc::SIGILL => "SIGILL (illegal instruction)",
        #[cfg(unix)]
        libc::SIGBUS => "SIGBUS (bus error)",
        _ => "unknown signal",
    };

    // Capture backtrace
    let bt = std::backtrace::Backtrace::force_capture();

    // Log to stderr (log macros may not be signal-safe)
    eprintln!("\n*** arf caught fatal signal: {} ***", signal_name);
    eprintln!("Backtrace:\n{}", bt);

    // Re-raise the signal to trigger the default handler
    unsafe {
        libc::raise(signum);
    }
}
