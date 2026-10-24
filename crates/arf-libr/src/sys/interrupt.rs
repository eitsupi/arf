use crate::functions::r_library;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

/// Pointer to R's interrupt pending flag, set during R initialization.
/// - Unix: points to `R_interrupts_pending` (c_int)
/// - Windows: points to `UserBreak` (Rboolean, c_int-sized)
pub(super) static R_INTERRUPT_FLAG: AtomicPtr<c_int> = AtomicPtr::new(std::ptr::null_mut());

/// Windows only: pointer to `R_interrupts_pending`, set during R initialization.
///
/// The Windows front-end break flag is `UserBreak` (stored in
/// [`R_INTERRUPT_FLAG`]), but when an interrupt arrives while
/// `R_interrupts_suspended` is set, `onintr()` defers it into
/// `R_interrupts_pending` — a different variable. Clearing a stale interrupt
/// on Windows therefore has to reset both.
#[cfg(windows)]
pub(super) static R_DEFERRED_INTERRUPT_FLAG: AtomicPtr<c_int> =
    AtomicPtr::new(std::ptr::null_mut());

/// Number of active `r_read_console` calls waiting for console input.
///
/// The Ctrl+C handler consults this to drop interrupts that arrive while no
/// R computation is running: if R's interrupt flag were set at that time,
/// the event polling done from the input-waiting loops (reedline idle
/// callback, pagers) could observe it and run `onintr()`, which longjmps to
/// R's top level straight through the Rust frames of those loops.
///
/// A counter rather than a flag because ReadConsole can nest: R code run
/// from the event polling of an outer read (e.g. a handler calling
/// `readline()`) enters `r_read_console` again, and the inner guard's drop
/// must not unmark the still-active outer read.
pub(super) static R_AWAITING_CONSOLE_INPUT: AtomicUsize = AtomicUsize::new(0);

/// RAII guard that sets `R_interrupts_suspended = TRUE` and restores the
/// previous value on drop.
///
/// While suspended, R's `onintr()` defers the interrupt (it re-sets
/// `R_interrupts_pending` and returns) instead of longjmping to R's top
/// level. This makes it safe to call R's event machinery from Rust
/// input-waiting loops even if a SIGINT handler on another thread sets the
/// interrupt flag at any point during the call — closing the race that the
/// [`R_AWAITING_CONSOLE_INPUT`] gate and the entry clears only narrow.
///
/// Unlike R's `END_SUSPEND_INTERRUPTS`, the drop deliberately does NOT call
/// `Rf_onintr()` for an interrupt that arrived while suspended: firing it
/// would longjmp through the Rust caller. Instead, the drop clears the
/// deferred pending flag while still suspended and only then restores the
/// previous suspension state, so a deferred interrupt can neither fire here
/// nor leak into the next R evaluation (interrupts make no sense at the
/// prompt). On Windows this matters doubly: onintr() defers into
/// `R_interrupts_pending`, which is a different variable from `UserBreak`.
///
/// `R_interrupts_suspended` is not part of R's documented API (see the
/// comment on `RLibrary::r_interrupts_suspended`); when the symbol is
/// unavailable the guard is a no-op, reverting to the narrowed-race behavior.
///
/// Known residual (accepted): a SIGINT handler on another thread that
/// observed `R_AWAITING_CONSOLE_INPUT == false` just before the transition
/// can still write the interrupt flag after the drop's clear, leaving a
/// stale flag that interrupts the start of the next evaluation once. This
/// is benign as long as suspension is active (no longjmp through Rust
/// frames; on builds where the symbol is unavailable the no-op guard leaves
/// the narrowed crash race described above) and requires a handler preempted
/// inside a several-instruction window plus input submitted within one idle
/// tick. Fully eliminating it would mean
/// blocking SIGINT on all threads and receiving it via sigwait on a
/// dedicated thread that shares a mutex with the ReadConsole transitions —
/// a redesign that is not worth it for a benign one-off; see the task
/// tracker for the design sketch if it ever becomes necessary.
struct SuspendRInterruptsGuard {
    ptr: *mut c_int,
    old: c_int,
}

impl SuspendRInterruptsGuard {
    fn new(ptr: *mut c_int) -> Self {
        let old = if ptr.is_null() {
            0
        } else {
            // SAFETY: ptr points to R's global variable, valid for the
            // process lifetime. Volatile access matches how the SIGINT
            // handler touches R globals.
            unsafe {
                let old = std::ptr::read_volatile(ptr);
                std::ptr::write_volatile(ptr, 1);
                old
            }
        };
        Self { ptr, old }
    }
}

impl Drop for SuspendRInterruptsGuard {
    fn drop(&mut self) {
        // Drop any interrupt deferred while suspended, before lifting the
        // suspension: after this point no R code runs that could convert or
        // defer flags, so the clear cannot race with R itself.
        clear_r_interrupt_pending();

        if !self.ptr.is_null() {
            // SAFETY: see SuspendRInterruptsGuard::new.
            unsafe {
                std::ptr::write_volatile(self.ptr, self.old);
            }
        }
    }
}

/// RAII guard that marks [`R_AWAITING_CONSOLE_INPUT`] for the duration of a
/// `r_read_console` call, covering every return path.
pub(super) struct AwaitConsoleInputGuard;

impl AwaitConsoleInputGuard {
    pub(super) fn new() -> Self {
        R_AWAITING_CONSOLE_INPUT.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

/// Temporarily allow SIGINT to reach an R evaluation initiated while
/// ReadConsole is active.
///
/// Interactive evaluation normally happens after the ReadConsole callback has
/// returned, so `R_AWAITING_CONSOLE_INPUT` is zero. A small number of prompt
/// actions evaluate R while the callback remains on the stack. In that case
/// this guard lets the regular R interrupt handler deliver Ctrl+C to a
/// protected top-level evaluation. It must be held only outside the
/// `R_ToplevelExec` callback so R's longjmp cannot skip this Rust destructor.
pub struct ConsoleEvaluationInterruptGuard {
    previous_count: usize,
    _not_send_or_sync: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl ConsoleEvaluationInterruptGuard {
    /// Temporarily mark ReadConsole as not waiting for input.
    ///
    /// # Safety
    /// The caller must be on R's main thread and must keep this guard outside
    /// any R top-level jump callback. The guarded work must enter R only
    /// through protected evaluation, so a longjmp cannot skip this guard's
    /// destructor. Guards must be scoped in LIFO order, without input-waiting
    /// loops or event polling while held.
    pub unsafe fn new() -> Self {
        let previous_count = R_AWAITING_CONSOLE_INPUT.swap(0, Ordering::AcqRel);
        Self {
            previous_count,
            _not_send_or_sync: std::marker::PhantomData,
        }
    }
}

impl Drop for ConsoleEvaluationInterruptGuard {
    fn drop(&mut self) {
        // Restore the input-waiting state before clearing flags. A concurrent
        // signal that observes the restored count is ignored by the handler;
        // clear flags from the protected evaluation before returning to input.
        // The preexisting race with a signal handler delayed between its
        // counter read and flag write can still leave one stale interrupt.
        R_AWAITING_CONSOLE_INPUT.store(self.previous_count, Ordering::Release);
        clear_r_interrupt_pending();
    }
}

impl Drop for AwaitConsoleInputGuard {
    fn drop(&mut self) {
        R_AWAITING_CONSOLE_INPUT.fetch_sub(1, Ordering::AcqRel);
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
/// to keep R's interactive windows responsive. arf does so from reedline's
/// idle callback in the interactive REPL and from the polling loop in
/// headless mode.
///
/// # Safety
/// R must be initialized before calling this function.
pub fn process_r_events() {
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };

    // Drop any pending interrupt before calling into R's event machinery.
    // This function is only called while no R computation is running: from
    // Rust input-waiting code (the reedline idle callback, the headless
    // loop, and once just before read_line()), so there is nothing to
    // interrupt. If the flag were left set, an interrupt observed here (by
    // R_ProcessEvents on Windows or R_checkActivity/R_runHandlers on Unix)
    // would call onintr(), which longjmps to R's top level straight through
    // the Rust frames of the caller (undefined behavior; in practice it
    // leaks a RefCell borrow and the session exits on the next
    // ReadConsole).
    clear_r_interrupt_pending();

    // Suspend interrupts for the duration of the call so that a flag set
    // concurrently (by a SIGINT handler running on another thread) cannot
    // trigger that longjmp either. See SuspendRInterruptsGuard.
    let _suspend_guard = SuspendRInterruptsGuard::new(lib.r_interrupts_suspended);

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

/// Process R events behind an R top-level jump boundary.
///
/// The callback payload contains only a raw library pointer and the callback
/// frame owns no Rust values with destructors, so an R error cannot unwind
/// through Rust-owned state.
pub fn process_r_events_protected() -> bool {
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return false,
    };
    clear_r_interrupt_pending();
    let suspend = SuspendRInterruptsGuard::new(lib.r_interrupts_suspended);
    let mut payload = lib as *const crate::functions::RLibrary as *mut std::ffi::c_void;
    let completed = unsafe {
        (lib.r_toplevelexec)(
            Some(process_r_events_callback),
            (&mut payload as *mut *mut std::ffi::c_void).cast(),
        )
    } != 0;
    drop(suspend);
    completed
}

unsafe extern "C-unwind" fn process_r_events_callback(payload: *mut std::ffi::c_void) {
    let library_ptr =
        unsafe { *(payload.cast::<*mut std::ffi::c_void>()) } as *const crate::functions::RLibrary;
    if library_ptr.is_null() {
        return;
    }
    let lib = unsafe { &*library_ptr };
    unsafe {
        (lib.r_processevents)();
        #[cfg(unix)]
        if !lib.r_inputhandlers.is_null() {
            let what = (lib.r_checkactivity)(0, 1);
            if !what.is_null() {
                (lib.r_runhandlers)(*lib.r_inputhandlers, what);
            }
        }
    }
}

/// Set R's interrupt pending flag to request computation interruption.
///
/// This is async-signal-safe: only performs an atomic load and a volatile write.
/// On Unix, sets `R_interrupts_pending = 1`.
/// On Windows, sets `UserBreak = TRUE` (1).
pub fn set_r_interrupt_pending() {
    let ptr = R_INTERRUPT_FLAG.load(Ordering::Acquire);
    if !ptr.is_null() {
        // SAFETY: ptr points to R's global variable, valid for the process lifetime.
        // Volatile write is async-signal-safe and prevents compiler elision.
        unsafe {
            std::ptr::write_volatile(ptr, 1);
        }
    }
}

/// Returns whether the R interrupt flag pointer is available.
///
/// When this returns `false`, [`set_r_interrupt_pending`] is a no-op.
pub fn is_r_interrupt_flag_available() -> bool {
    !R_INTERRUPT_FLAG.load(Ordering::Acquire).is_null()
}

/// Returns whether R is currently inside `ReadConsole` waiting for input.
///
/// This is async-signal-safe (a single atomic load). The Ctrl+C handler uses
/// it to drop interrupts that arrive while no R computation is running; see
/// [`R_AWAITING_CONSOLE_INPUT`] for why setting the flag then is unsafe.
pub fn is_r_awaiting_console_input() -> bool {
    R_AWAITING_CONSOLE_INPUT.load(Ordering::Acquire) > 0
}

/// Clear R's interrupt pending flag.
///
/// Called at the start of every `ReadConsole` invocation (including nested
/// prompts such as `readline()`, `browser()`, etc.) to prevent stale
/// interrupt flags from triggering on the next evaluation.
///
/// On Windows this clears both `UserBreak` (the front-end break flag) and
/// `R_interrupts_pending` (where `onintr()` defers an interrupt that arrives
/// while `R_interrupts_suspended` is set). On Unix they are the same flag.
pub(super) fn clear_r_interrupt_pending() {
    let ptr = R_INTERRUPT_FLAG.load(Ordering::Acquire);
    if !ptr.is_null() {
        unsafe {
            std::ptr::write_volatile(ptr, 0);
        }
    }

    #[cfg(windows)]
    {
        let ptr = R_DEFERRED_INTERRUPT_FLAG.load(Ordering::Acquire);
        if !ptr.is_null() {
            unsafe {
                std::ptr::write_volatile(ptr, 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AwaitConsoleInputGuard, ConsoleEvaluationInterruptGuard, is_r_awaiting_console_input,
    };

    #[test]
    fn protected_evaluation_restores_nested_console_waits() {
        assert!(!is_r_awaiting_console_input());
        let outer = AwaitConsoleInputGuard::new();
        let inner = AwaitConsoleInputGuard::new();
        assert!(is_r_awaiting_console_input());
        {
            // SAFETY: this test runs no R code or event polling. It checks the
            // input-count transitions with guards scoped in LIFO order.
            let _evaluation = unsafe { ConsoleEvaluationInterruptGuard::new() };
            assert!(!is_r_awaiting_console_input());
            {
                let _nested_read = AwaitConsoleInputGuard::new();
                assert!(is_r_awaiting_console_input());
            }
            assert!(!is_r_awaiting_console_input());
        }
        assert!(is_r_awaiting_console_input());
        drop(inner);
        assert!(is_r_awaiting_console_input());
        drop(outer);
        assert!(!is_r_awaiting_console_input());
    }
}
