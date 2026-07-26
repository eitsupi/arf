use super::arf_println;

/// RAII guard that sets IPC alternate mode on creation and restores the previous state on drop.
///
/// Pagers that enter crossterm's alternate screen must be wrapped with this guard
/// so that IPC requests are rejected immediately instead of hanging.
/// The drop guard also restores the state on panic unwind (where the panic strategy permits it).
struct IpcAlternateGuard {
    was_alternate: bool,
}

impl IpcAlternateGuard {
    fn new() -> Self {
        let was_alternate = crate::ipc::is_in_alternate_mode();
        crate::ipc::set_in_alternate_mode(true);
        Self { was_alternate }
    }
}

impl Drop for IpcAlternateGuard {
    fn drop(&mut self) {
        crate::ipc::set_in_alternate_mode(self.was_alternate);
    }
}

/// Run a closure with IPC alternate mode enabled, restoring the previous state afterward.
pub(super) fn with_ipc_alternate_guard<R>(f: impl FnOnce() -> R) -> R {
    let _guard = IpcAlternateGuard::new();
    f()
}

/// Run the help browser pager, wrapping with IPC alternate mode.
pub(super) fn run_pager_help_browser(query: &str) {
    let help_result = with_ipc_alternate_guard(|| crate::pager::run_help_browser(query));
    if let Err(e) = help_result {
        arf_println!("Error in help browser: {}", e);
    }
}

/// Run the history browser pager, wrapping with IPC alternate mode.
pub(super) fn run_pager_history_browser(path: &std::path::Path, mode: crate::pager::HistoryDbMode) {
    let browser_result = with_ipc_alternate_guard(|| crate::pager::run_history_browser(path, mode));

    match browser_result {
        Ok(crate::pager::HistoryBrowserResult::Copied(cmd)) => {
            let display = crate::pager::text_utils::truncate_to_width(&cmd, 60);
            arf_println!("Copied: {}", display);
        }
        Ok(crate::pager::HistoryBrowserResult::Cancelled) => {}
        Err(e) => {
            arf_println!("Error: {}", e);
        }
    }
}
