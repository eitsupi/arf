use super::arf_println;
use arf_harp::help_bridge::{PreparedHelpRequest, drain_prepared_help_requests};
use std::io;

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

/// Display pending help requests at a top-level prompt.
///
/// Each complete pager/selector sequence is guarded as one IPC alternate-mode
/// operation. If terminal setup or display fails, the guard is dropped before
/// all pages from that request are printed through ordinary console output.
pub(super) fn process_pending_help_requests() {
    process_prepared_help_requests(
        drain_prepared_help_requests(),
        |request| with_ipc_alternate_guard(|| crate::pager::display_prepared_help_request(request)),
        |request, error| {
            log::error!("Prepared help pager failed: {error}");
            fallback_prepared_help_request(request, error);
        },
    );
}

fn process_prepared_help_requests(
    requests: Vec<PreparedHelpRequest>,
    mut display: impl FnMut(&PreparedHelpRequest) -> io::Result<()>,
    mut fallback: impl FnMut(&PreparedHelpRequest, &io::Error),
) {
    for request in requests {
        if let Err(error) = display(&request) {
            fallback(&request, &error);
        }
    }
}

fn fallback_prepared_help_request(request: &PreparedHelpRequest, error: &io::Error) {
    arf_println!("Interactive help display failed: {error}");
    for page in &request.pages {
        arf_println!("{}::{}", page.package, page.display_topic);
        println!("{}", page.markdown);
    }
}

/// Run the help browser pager, wrapping with IPC alternate mode.
pub(super) fn run_pager_help_browser(query: &str) {
    let help_result = with_ipc_alternate_guard(|| crate::pager::run_help_browser(query));
    if let Err(e) = help_result {
        arf_println!("Error in help browser: {}", e);
    }
}

/// Run the history browser pager, wrapping with IPC alternate mode.
pub(super) fn run_pager_history_browser(
    store: &crate::history::HistoryStore,
    mode: crate::pager::HistoryDbMode,
) {
    let browser_result =
        with_ipc_alternate_guard(|| crate::pager::run_history_browser(store, mode));

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

#[cfg(test)]
mod tests {
    use super::*;
    use arf_harp::help_bridge::PreparedHelpPage;
    use std::path::PathBuf;

    fn request(topic: &str) -> PreparedHelpRequest {
        PreparedHelpRequest {
            topic: topic.to_string(),
            pages: vec![PreparedHelpPage {
                package_dir: PathBuf::from("/library/base"),
                package: "base".to_string(),
                display_topic: topic.to_string(),
                help_key: topic.to_string(),
                markdown: format!("Markdown for {topic}"),
            }],
        }
    }

    #[test]
    fn pager_failure_falls_back_after_ui_closure_and_continues_fifo() {
        let requests = vec![request("mean"), request("sum")];
        let events = std::cell::RefCell::new(Vec::new());
        process_prepared_help_requests(
            requests,
            |request| {
                events
                    .borrow_mut()
                    .push(format!("display:{}", request.topic));
                if request.topic == "mean" {
                    Err(io::Error::other("terminal unavailable"))
                } else {
                    Ok(())
                }
            },
            |request, _| {
                events
                    .borrow_mut()
                    .push(format!("fallback:{}", request.topic))
            },
        );
        assert_eq!(
            events.into_inner(),
            ["display:mean", "fallback:mean", "display:sum"]
        );
    }
}
