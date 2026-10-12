//! SIGINT handling that forwards Ctrl+C to R.

/// Install the Ctrl+C handler that forwards interrupts to R.
///
/// Without this, Ctrl+C during R evaluation terminates the process: we set
/// R_SignalHandlers = 0, so R installs no SIGINT handler itself and the
/// default action kills the process (on Unix, R_SelectEx only installs a
/// temporary handler while blocked in select(), leaving a fatal window
/// between calls; on Windows, STATUS_CONTROL_C_EXIT).
/// The handler sets R's interrupt flag (R_interrupts_pending / UserBreak),
/// which R checks periodically (R_CheckUserInterrupt, R_SelectEx) and
/// turns into an interrupt condition via onintr() — but only while R is
/// evaluating: while ReadConsole waits for input there is nothing to
/// interrupt, and a flag observed by the event polling done from the
/// input-waiting loops would make onintr() longjmp through their Rust
/// frames, so the handler drops the signal instead.
///
/// On Unix, call this BEFORE R initialization: startup profiles run inside
/// setup_Rmainloop, so installing later leaves a window where Ctrl+C during
/// a slow .Rprofile kills the process. The interrupt flag pointer is
/// resolved early in initialization (before profiles are evaluated); until
/// then the handler is a no-op. If the flag is still unavailable after
/// initialization, call [`restore_default_sigint_handler`] so Ctrl+C is not
/// swallowed forever. On Windows, profiles are sourced manually after
/// initialization, so call this between the two (gated on flag
/// availability).
pub(crate) fn install_r_interrupt_handler() {
    // Unix: register a SIGINT-only sigaction instead of using ctrlc.
    // The workspace builds ctrlc with the "termination" feature (for
    // headless graceful shutdown), so ctrlc::set_handler would also
    // capture SIGTERM/SIGHUP and an interactive session could no
    // longer be terminated by them.
    #[cfg(unix)]
    {
        use nix::sys::signal;

        extern "C" fn handle_sigint(_signum: std::ffi::c_int) {
            // Async-signal-safe: one atomic load, then an atomic load
            // plus a volatile write. Must not panic or allocate.
            if !arf_libr::is_r_awaiting_console_input() {
                arf_libr::set_r_interrupt_pending();
            }
        }

        // SA_RESTART so blocking syscalls interrupted by the signal
        // are transparently restarted (as ctrlc does).
        let action = signal::SigAction::new(
            signal::SigHandler::Handler(handle_sigint),
            signal::SaFlags::SA_RESTART,
            signal::SigSet::empty(),
        );
        // SAFETY: handle_sigint is async-signal-safe (see above).
        if let Err(e) = unsafe { signal::sigaction(signal::Signal::SIGINT, &action) } {
            log::warn!("Could not set Ctrl+C handler: {e}");
        }
    }

    #[cfg(windows)]
    if let Err(e) = ctrlc::set_handler(|| {
        if !arf_libr::is_r_awaiting_console_input() {
            arf_libr::set_r_interrupt_pending();
        }
    }) {
        log::warn!("Could not set Ctrl+C handler: {e}");
    }
}

/// Restore the default SIGINT disposition (terminate the process).
///
/// Used when R's interrupt flag turns out to be unavailable after R
/// initialization: the handler installed by [`install_r_interrupt_handler`]
/// can never forward interrupts then, and would swallow Ctrl+C forever.
#[cfg(unix)]
pub(crate) fn restore_default_sigint_handler() {
    use nix::sys::signal;

    let action = signal::SigAction::new(
        signal::SigHandler::SigDfl,
        signal::SaFlags::empty(),
        signal::SigSet::empty(),
    );
    // SAFETY: restores the default disposition; no handler code involved.
    if let Err(e) = unsafe { signal::sigaction(signal::Signal::SIGINT, &action) } {
        log::warn!("Could not restore default Ctrl+C handler: {e}");
    }
}
