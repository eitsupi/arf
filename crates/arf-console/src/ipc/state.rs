//! Process-global flags and channels shared by the IPC request handlers.

use super::protocol::IpcRequest;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

/// Shutdown flag for headless mode. When set, the headless event loop exits.
/// Not set in REPL mode (shutdown is only available in headless mode).
static HEADLESS_SHUTDOWN: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Channel receiver for IPC requests (server → main thread).
/// Wrapped in Option so it can be replaced on restart.
static IPC_RECEIVER: OnceLock<Mutex<Option<std::sync::mpsc::Receiver<IpcRequest>>>> =
    OnceLock::new();

/// Whether R is currently busy evaluating (not waiting for input).
static R_IS_AT_PROMPT: OnceLock<AtomicBool> = OnceLock::new();

/// Whether the REPL is in an alternate mode (shell mode, history browser, help browser)
/// where the idle callback is not running and IPC requests would hang.
static IN_ALTERNATE_MODE: AtomicBool = AtomicBool::new(false);

/// Break signal shared with reedline to interrupt `read_line()`.
static BREAK_SIGNAL: OnceLock<Arc<AtomicBool>> = OnceLock::new();

pub(super) fn r_is_at_prompt() -> &'static AtomicBool {
    R_IS_AT_PROMPT.get_or_init(|| AtomicBool::new(false))
}

/// Mark that R is now at the command prompt (idle, ready for input).
pub fn set_r_at_prompt(at_prompt: bool) {
    r_is_at_prompt().store(at_prompt, Ordering::Release);
}

/// Set whether the REPL is in an alternate mode (shell, history browser, etc.).
pub fn set_in_alternate_mode(active: bool) {
    IN_ALTERNATE_MODE.store(active, Ordering::Release);
}

/// Check if the REPL is in an alternate mode.
pub fn is_in_alternate_mode() -> bool {
    IN_ALTERNATE_MODE.load(Ordering::Acquire)
}

/// Get or create the break signal for reedline integration.
/// Pass the returned `Arc` to `Reedline::with_break_signal()`.
pub fn break_signal() -> Arc<AtomicBool> {
    BREAK_SIGNAL
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// Fire the break signal to interrupt reedline's `read_line()` loop.
pub(super) fn fire_break_signal() {
    if let Some(signal) = BREAK_SIGNAL.get() {
        signal.store(true, Ordering::Relaxed);
    }
}

/// Register the headless shutdown flag so `shutdown` IPC method can trigger it.
///
/// Must be called before `start_server()` in headless mode. In REPL mode
/// this is never called, so `shutdown` requests return METHOD_NOT_FOUND.
pub fn set_headless_shutdown(flag: Arc<AtomicBool>) {
    let _ = HEADLESS_SHUTDOWN.set(flag);
}

/// Try to trigger headless shutdown. Returns true if the flag was set.
///
/// Called from the server thread when a `shutdown` request arrives.
/// Returns false if not in headless mode (flag not registered).
pub fn trigger_headless_shutdown() -> bool {
    if let Some(flag) = HEADLESS_SHUTDOWN.get() {
        flag.store(true, Ordering::Release);
        true
    } else {
        false
    }
}

pub fn has_headless_shutdown_handler() -> bool {
    HEADLESS_SHUTDOWN.get().is_some()
}

pub fn headless_shutdown_requested() -> bool {
    HEADLESS_SHUTDOWN
        .get()
        .is_some_and(|flag| flag.load(Ordering::Acquire))
}

/// Store the receiver for polling from the idle callback.
///
/// If the receiver slot already exists (from a previous stop/start), its
/// inner value is replaced.
pub(super) fn install_receiver(rx: std::sync::mpsc::Receiver<IpcRequest>) -> std::io::Result<()> {
    match IPC_RECEIVER.get() {
        Some(existing) => {
            *existing.lock().unwrap() = Some(rx);
        }
        None => {
            IPC_RECEIVER
                .set(Mutex::new(Some(rx)))
                .map_err(|_| std::io::Error::other("IPC receiver already initialized"))?;
        }
    }
    Ok(())
}

/// Drop the receiver so the server thread's `mpsc::send` fails.
pub(super) fn clear_receiver() {
    if let Some(receiver) = IPC_RECEIVER.get() {
        *receiver.lock().unwrap() = None;
    }
}

/// Hand every queued request to `handle`, without blocking.
///
/// Returns `true` if at least one request was handled. Returns `false`
/// without handling anything when IPC was never started, was stopped, or the
/// receiver is currently locked by another caller.
pub(super) fn drain_requests(mut handle: impl FnMut(IpcRequest)) -> bool {
    let receiver = match IPC_RECEIVER.get() {
        Some(r) => r,
        None => return false, // IPC not started
    };

    let rx = match receiver.try_lock() {
        Ok(rx) => rx,
        Err(_) => return false,
    };

    let rx = match rx.as_ref() {
        Some(rx) => rx,
        None => return false, // IPC stopped
    };

    let mut processed = false;
    // Do not let a continuously writing client monopolize the editor's idle
    // callback. Remaining requests retain channel FIFO order for the next poll.
    for request in rx.try_iter().take(128) {
        processed = true;
        handle(request);
    }
    processed
}

pub(super) fn try_receive_request() -> Option<IpcRequest> {
    let receiver = IPC_RECEIVER.get()?;
    let guard = receiver.try_lock().ok()?;
    guard.as_ref()?.try_recv().ok()
}
