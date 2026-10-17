//! IPC request handling for headless mode.
//!
//! The R mainloop reads request-owned input without an interactive editor.

use super::capture;
use super::history_query::set_history_store;
use super::policy;
use super::protocol::{INVALID_PARAMS, IpcMethod, IpcRequest, IpcResponse, UserInputResult};
use super::session_result::collect_session_result;
use super::state::{r_is_at_prompt, try_receive_request};
use crate::history::{HistoryExtraInfo, HistoryStore};
use std::sync::{Mutex, OnceLock, atomic::Ordering};

/// History backend for headless mode. When set, evaluated commands are
/// persisted to the same SQLite history database used by the REPL.
static HEADLESS_HISTORY: OnceLock<HistoryStore> = OnceLock::new();

enum ActiveKind {
    Evaluate(tokio::sync::oneshot::Sender<IpcResponse>),
    UserInput,
}

struct ActiveRequest {
    code: String,
    offset: usize,
    kind: ActiveKind,
    reset_generation: u64,
    exhausted: bool,
    continuation_eof_sent: bool,
}

static ACTIVE_REQUEST: Mutex<Option<ActiveRequest>> = Mutex::new(None);

/// Set the history backend for headless mode.
///
/// Evaluated commands are persisted to the SQLite history database.
pub fn set_headless_history(history: HistoryStore) {
    let ipc_history = history.clone();
    if HEADLESS_HISTORY.set(history).is_err() {
        log::warn!(
            "Headless history backend already initialized; ignoring duplicate set_headless_history call"
        );
    }
    set_history_store(ipc_history);
}

/// Save a command to the headless history database, if configured.
///
/// Errors are logged but never propagated — history saving must not
/// interfere with IPC response delivery.
fn save_to_headless_history(code: &str, exit_status: Option<i64>) {
    let Some(h) = HEADLESS_HISTORY.get() else {
        return;
    };
    let mut item = reedline::HistoryItem::from_command_line(code);
    item.start_timestamp = Some(chrono::Utc::now());
    item.hostname = Some(gethostname::gethostname().to_string_lossy().into_owned());
    item.cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    item.exit_status = exit_status;
    item.session_id = HEADLESS_HISTORY.get().and_then(HistoryStore::session);
    if let Err(e) = h.save_known(item, HistoryExtraInfo::default()) {
        log::warn!("Failed to save headless history: {}", e);
    }
}

/// Read request-owned input through R's actual top-level loop.
pub fn headless_read_console(
    prompt: &str,
    info: arf_libr::ReadConsolePromptInfo,
) -> Option<arf_libr::ReadConsoleInput> {
    if info.hist == 0 {
        return None;
    }
    if !info.prompt_options_are_valid {
        log::error!("Headless R prompt and continuation options are missing or ambiguous");
        set_headless_shutdown();
        shutdown_before_eof(1);
    }
    if is_browse_prompt(prompt) {
        return None;
    }
    match arf_libr::frame_depth_protected() {
        Ok(0) => {}
        Ok(depth) if depth > 0 => return None,
        Ok(depth) => {
            log::error!("R returned an invalid evaluation frame depth: {depth}");
            set_headless_shutdown();
            shutdown_before_eof(1);
        }
        Err(error) => {
            log::error!("Could not determine whether R input is nested: {error}");
            set_headless_shutdown();
            shutdown_before_eof(1);
        }
    }
    if !info.is_main_prompt && !info.is_continuation {
        log::error!("Headless R requested an unrecognized prompt: {prompt:?}");
        set_headless_shutdown();
        shutdown_before_eof(1);
    }
    if super::state::headless_shutdown_requested() {
        finish_completed_request_before_shutdown(info.is_main_prompt);
        shutdown_before_eof(0);
    }

    loop {
        if super::state::headless_shutdown_requested() {
            finish_completed_request_before_shutdown(info.is_main_prompt);
            shutdown_before_eof(0);
        }
        if ACTIVE_REQUEST.lock().unwrap().is_some() {
            let current_generation = arf_libr::reset_console_generation();
            let should_finish = {
                let mut active = ACTIVE_REQUEST.lock().unwrap();
                let Some(request) = active.as_mut() else {
                    continue;
                };
                if request.reset_generation != current_generation {
                    request.offset = request.code.len();
                    request.exhausted = true;
                }
                info.is_main_prompt && request.exhausted
            };
            if should_finish {
                finish_active_request();
                continue;
            }
            let continuation_eof = if info.is_continuation {
                let mut active = ACTIVE_REQUEST.lock().unwrap();
                active.as_mut().is_some_and(|request| {
                    if !request.exhausted {
                        return false;
                    }
                    if request.continuation_eof_sent {
                        return true;
                    }
                    request.continuation_eof_sent = true;
                    false
                })
            } else {
                false
            };
            if continuation_eof {
                log::error!("R requested continuation input again after request EOF");
                set_headless_shutdown();
                shutdown_before_eof(1);
            }
            let exhausted_continuation = info.is_continuation
                && ACTIVE_REQUEST
                    .lock()
                    .unwrap()
                    .as_ref()
                    .is_some_and(|request| request.exhausted);
            if exhausted_continuation {
                // Let R abandon its incomplete parser buffer and return to
                // the main prompt before completing this request.
                return None;
            }
            if let Some(chunk) = next_active_chunk(info.buffer_capacity, info.is_main_prompt) {
                return Some(chunk);
            }
            log::error!("R ReadConsole buffer cannot fit the next request chunk");
            set_headless_shutdown();
            shutdown_before_eof(1);
        } else if info.is_main_prompt
            && let Some(request) = try_receive_request()
        {
            match start_request(request) {
                RequestStart::Input => {
                    if let Some(chunk) =
                        next_active_chunk(info.buffer_capacity, info.is_main_prompt)
                    {
                        return Some(chunk);
                    }
                    log::error!("R ReadConsole buffer cannot fit the first request chunk");
                    set_headless_shutdown();
                    shutdown_before_eof(1);
                }
                RequestStart::Continue => continue,
            }
        }
        arf_libr::process_r_events_protected();
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn finish_completed_request_before_shutdown(is_main_prompt: bool) {
    let completed = is_main_prompt
        && ACTIVE_REQUEST
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|request| request.exhausted);
    if completed {
        finish_active_request();
    }
}

enum RequestStart {
    Input,
    Continue,
}

fn start_request(request: IpcRequest) -> RequestStart {
    let IpcRequest { method, reply } = request;
    match method {
        IpcMethod::Evaluate { code, visible, .. } => {
            if code.contains('\0') {
                let _ = reply.send(IpcResponse::error(
                    INVALID_PARAMS,
                    "R code contains a NUL byte".to_string(),
                ));
                return RequestStart::Continue;
            }
            if !visible {
                if let Some(rejection) = policy::policy_reject(&code) {
                    let _ = reply.send(rejection);
                    return RequestStart::Continue;
                }
                r_is_at_prompt().store(false, Ordering::Release);
                let result = capture::evaluate_with_capture(&code, false);
                r_is_at_prompt().store(true, Ordering::Release);
                let exit_status = if result.error.is_some() { 1 } else { 0 };
                let _ = reply.send(IpcResponse::Evaluate(result));
                if !code.trim().is_empty() {
                    save_to_headless_history(&code, Some(exit_status));
                }
                return RequestStart::Continue;
            }
            arf_libr::start_ipc_capture(true);
            let generation = arf_libr::reset_console_generation();
            r_is_at_prompt().store(false, Ordering::Release);
            *ACTIVE_REQUEST.lock().unwrap() = Some(ActiveRequest {
                code,
                offset: 0,
                kind: ActiveKind::Evaluate(reply),
                reset_generation: generation,
                exhausted: false,
                continuation_eof_sent: false,
            });
            RequestStart::Input
        }
        IpcMethod::UserInput { code } => {
            if code.contains('\0') {
                let _ = reply.send(IpcResponse::error(
                    INVALID_PARAMS,
                    "R code contains a NUL byte".to_string(),
                ));
                return RequestStart::Continue;
            }
            let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: true }));
            let generation = arf_libr::reset_console_generation();
            r_is_at_prompt().store(false, Ordering::Release);
            *ACTIVE_REQUEST.lock().unwrap() = Some(ActiveRequest {
                code,
                offset: 0,
                kind: ActiveKind::UserInput,
                reset_generation: generation,
                exhausted: false,
                continuation_eof_sent: false,
            });
            RequestStart::Input
        }
        IpcMethod::Session => {
            let result = collect_session_result(true, "");
            let _ = reply.send(IpcResponse::Session(Box::new(result)));
            RequestStart::Continue
        }
    }
}

fn next_active_chunk(capacity: usize, is_main_prompt: bool) -> Option<arf_libr::ReadConsoleInput> {
    let mut guard = ACTIVE_REQUEST.lock().unwrap();
    let request = guard.as_mut()?;
    if is_main_prompt {
        while request.offset < request.code.len()
            && is_horizontal_ascii_whitespace(request.code.as_bytes()[request.offset])
        {
            request.offset += 1;
        }
        if request.offset == request.code.len() {
            request.exhausted = true;
            return Some(arf_libr::ReadConsoleInput {
                text: String::new(),
                has_more: false,
            });
        }
    }
    let remaining = &request.code[request.offset..];
    let final_capacity = capacity.saturating_sub(1);
    let (end, has_more) = if remaining.len() <= final_capacity {
        (remaining.len(), false)
    } else {
        let mut end = capacity.min(remaining.len());
        while end > 0 && !remaining.is_char_boundary(end) {
            end -= 1;
        }
        if end == 0 {
            log::error!("R ReadConsole buffer cannot fit one UTF-8 character");
            return None;
        }
        let mut deferred_end = end;
        while deferred_end > 0
            && is_horizontal_ascii_whitespace(remaining.as_bytes()[deferred_end - 1])
        {
            deferred_end -= 1;
        }
        if deferred_end > 0 {
            (deferred_end, true)
        } else {
            (end, true)
        }
    };
    let text = remaining[..end].to_owned();
    request.offset += end;
    request.exhausted = !has_more;
    Some(arf_libr::ReadConsoleInput { text, has_more })
}

fn is_horizontal_ascii_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

fn finish_active_request() {
    let request = ACTIVE_REQUEST.lock().unwrap().take();
    let Some(request) = request else { return };
    let failed = request.reset_generation != arf_libr::reset_console_generation();
    if let ActiveKind::Evaluate(reply) = request.kind {
        let (stdout, stderr) = arf_libr::finish_ipc_capture();
        let _ = reply.send(IpcResponse::Evaluate(super::protocol::EvaluateResult {
            stdout,
            stderr,
            value: None,
            error: None,
        }));
    }
    if !request.code.trim().is_empty() {
        save_to_headless_history(&request.code, Some(if failed { 1 } else { 0 }));
    }
    r_is_at_prompt().store(true, Ordering::Release);
}

fn shutdown_before_eof(exit_code: i32) -> ! {
    let pending = { ACTIVE_REQUEST.lock().unwrap().take() };
    let _ = arf_libr::finish_ipc_capture();
    if let Some(request) = pending {
        if let ActiveKind::Evaluate(reply) = request.kind {
            let _ = reply.send(IpcResponse::error(
                super::protocol::R_NOT_AT_PROMPT,
                "Headless session is shutting down".to_string(),
            ));
        }
        if !request.code.trim().is_empty() {
            save_to_headless_history(&request.code, Some(1));
        }
    }
    if let Some(history) = HEADLESS_HISTORY.get()
        && let Err(error) = history.sync()
    {
        log::warn!("Failed to sync headless history during shutdown: {error}");
    }
    arf_libr::flush_reprex_buffer();
    use std::io::Write;
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    crate::ipc::stop_server();
    crate::pid_file::cleanup_registered_ipc_pid_file();
    std::process::exit(exit_code);
}

fn set_headless_shutdown() {
    super::state::trigger_headless_shutdown();
}

fn is_browse_prompt(prompt: &str) -> bool {
    let prompt = prompt.trim_end();
    let Some(number) = prompt
        .strip_prefix("Browse[")
        .and_then(|tail| tail.strip_suffix("]>"))
    else {
        return false;
    };
    !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
}
