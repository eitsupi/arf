//! IPC server for external tool access to the arf R session.
//!
//! Provides a JSON-RPC 2.0 interface over Unix sockets (or named pipes on Windows)
//! for AI agents, vscode-R, and other tools to interact with R.
//!
//! ## Mutual exclusion between console and IPC input
//!
//! All IPC operations that could conflict with console input (user_input,
//! evaluate with visible=true/false) are routed through reedline's ExternalBreak
//! mechanism. This allows the REPL to check the editor buffer before accepting
//! or rejecting the operation:
//!
//! 1. Idle callback receives IPC request → stores in `PENDING_IPC_OPERATION` → fires break signal
//! 2. Reedline returns `Signal::ExternalBreak(buffer)` with current editor buffer
//! 3. REPL checks buffer: empty → accept operation, non-empty → reject with `USER_IS_TYPING`

mod capture;
pub mod client;
pub mod protocol;
pub mod server;
pub mod session;

use chrono::TimeZone;
use protocol::{
    EvaluateResult, HistoryEntry, HistoryParams, HistoryResult, INPUT_ALREADY_PENDING, IpcMethod,
    IpcRequest, IpcResponse, R_BUSY, R_NOT_AT_PROMPT, RSessionInfo, SessionResult, USER_IS_TYPING,
    UserInputResult,
};
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex, OnceLock,
    atomic::{AtomicBool, Ordering},
};

/// Shutdown flag for headless mode. When set, the headless event loop exits.
/// Not set in REPL mode (shutdown is only available in headless mode).
static HEADLESS_SHUTDOWN: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// History backend for headless mode. When set, evaluated commands are
/// persisted to the same SQLite history database used by the REPL.
static HEADLESS_HISTORY: OnceLock<Mutex<reedline::SqliteBackedHistory>> = OnceLock::new();

/// History database path and session ID, shared between REPL and headless modes.
/// Used by the `history` IPC method to open a read-only connection for queries.
static HISTORY_DB_INFO: OnceLock<(PathBuf, Option<reedline::HistorySessionId>)> = OnceLock::new();

/// Channel receiver for IPC requests (server → main thread).
/// Wrapped in Option so it can be replaced on restart.
static IPC_RECEIVER: OnceLock<Mutex<Option<std::sync::mpsc::Receiver<IpcRequest>>>> =
    OnceLock::new();

/// Pending IPC operation waiting for ExternalBreak to check the editor buffer.
/// At most one operation can be pending at a time.
static PENDING_IPC_OPERATION: OnceLock<Mutex<Option<PendingIpcOperation>>> = OnceLock::new();

fn pending_ipc_operation() -> &'static Mutex<Option<PendingIpcOperation>> {
    PENDING_IPC_OPERATION.get_or_init(|| Mutex::new(None))
}

/// Whether R is currently busy evaluating (not waiting for input).
static R_IS_AT_PROMPT: OnceLock<AtomicBool> = OnceLock::new();

/// Whether the REPL is in an alternate mode (shell mode, history browser, help browser)
/// where the idle callback is not running and IPC requests would hang.
static IN_ALTERNATE_MODE: AtomicBool = AtomicBool::new(false);

fn r_is_at_prompt() -> &'static AtomicBool {
    R_IS_AT_PROMPT.get_or_init(|| AtomicBool::new(false))
}

/// Set whether the REPL is in an alternate mode (shell, history browser, etc.).
pub fn set_in_alternate_mode(active: bool) {
    IN_ALTERNATE_MODE.store(active, Ordering::Release);
}

/// Check if the REPL is in an alternate mode.
pub fn is_in_alternate_mode() -> bool {
    IN_ALTERNATE_MODE.load(Ordering::Acquire)
}

/// Break signal shared with reedline to interrupt `read_line()`.
static BREAK_SIGNAL: OnceLock<Arc<AtomicBool>> = OnceLock::new();

/// Pending visible evaluate: reply channel waiting for REPL evaluation to complete.
/// When set, the WriteConsoleEx callback is capturing output. The reply is sent
/// once R returns to the prompt (detected by `check_visible_eval_completion`).
static PENDING_VISIBLE_EVAL: OnceLock<Mutex<Option<PendingVisibleEval>>> = OnceLock::new();

struct PendingVisibleEval {
    reply: tokio::sync::oneshot::Sender<IpcResponse>,
    /// Absolute deadline — best-effort aligned with the server-side timeout so that
    /// REPL-side cleanup should not outlive the server's oneshot wait under normal conditions.
    deadline: std::time::Instant,
    /// Original timeout duration, kept for diagnostic messages.
    timeout: std::time::Duration,
}

fn pending_visible_eval() -> &'static Mutex<Option<PendingVisibleEval>> {
    PENDING_VISIBLE_EVAL.get_or_init(|| Mutex::new(None))
}

/// An IPC operation waiting for ExternalBreak buffer check.
pub struct PendingIpcOperation {
    pub kind: PendingIpcKind,
    pub code: String,
}

/// The kind of pending IPC operation, carrying its reply channel.
pub enum PendingIpcKind {
    /// Silent evaluate: run R code in the REPL thread with output capture.
    /// Reply is sent immediately after evaluation completes.
    SilentEvaluate {
        reply: tokio::sync::oneshot::Sender<IpcResponse>,
    },
    /// Visible evaluate: inject code into REPL, capture output via WriteConsoleEx.
    /// Reply is deferred until R returns to the prompt.
    VisibleEvaluate {
        reply: tokio::sync::oneshot::Sender<IpcResponse>,
        timeout: std::time::Duration,
    },
    /// User input: inject code into REPL as if the user typed it.
    /// Reply is sent when the operation is accepted or rejected.
    UserInput {
        reply: tokio::sync::oneshot::Sender<IpcResponse>,
    },
}

/// Get or create the break signal for reedline integration.
/// Pass the returned `Arc` to `Reedline::with_break_signal()`.
pub fn break_signal() -> Arc<AtomicBool> {
    BREAK_SIGNAL
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// Start the IPC server and set up channels.
///
/// Called from `main.rs` when `--with-ipc` is specified,
/// or from `:ipc start` meta command.
///
/// If `bind` is `Some`, the server binds to the given path instead of the
/// default PID-based path.
pub fn start_server(
    bind: Option<&str>,
    log_file: Option<String>,
    history_session_id: Option<i64>,
) -> std::io::Result<session::SessionInfo> {
    let (tx, rx) = std::sync::mpsc::channel();

    // Initialize pending operation storage
    let _ = pending_ipc_operation();

    // Capture the start time once so both the session file and in-memory
    // cache use the same value.
    let started_at = chrono::Local::now().to_rfc3339();

    // Start the server thread first; only update the receiver after
    // confirming that the server bound successfully, so a failed start
    // doesn't break an already-running server's channel.
    let session = server::start_server(tx, bind, &started_at, log_file, history_session_id)?;

    // Note: session metadata is now cached inside server::start_server()
    // right after bind confirmation, before the server can serve any request.

    // Store receiver for polling from idle callback.
    // If OnceLock is already set (from a previous stop/start), replace the inner value.
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

    Ok(session)
}

/// Stop the IPC server (cleanup on exit).
///
/// Clears any in-flight IPC state (pending operations, active capture)
/// so the REPL doesn't get stuck in a pending/capturing state.
pub fn stop_server() {
    // Drop the receiver so the server thread's mpsc::send fails
    if let Some(receiver) = IPC_RECEIVER.get() {
        *receiver.lock().unwrap() = None;
    }

    // Reply to any pending operation with a cancellation error
    if let Some(pending) = take_pending_ipc_operation() {
        match pending.kind {
            PendingIpcKind::SilentEvaluate { reply }
            | PendingIpcKind::VisibleEvaluate { reply, .. }
            | PendingIpcKind::UserInput { reply } => {
                let _ = reply.send(IpcResponse::error(
                    R_NOT_AT_PROMPT,
                    "IPC server is shutting down".to_string(),
                ));
            }
        }
    }

    // Finalize any active visible eval capture.
    // Use blocking lock — stop_server is a final cleanup and can afford to wait.
    if let Some(pending) = pending_visible_eval()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        let (stdout, stderr) = arf_libr::finish_ipc_capture();
        let _ = pending.reply.send(IpcResponse::error(
            R_NOT_AT_PROMPT,
            format!(
                "IPC server shut down during visible evaluate (stdout: {} bytes, stderr: {} bytes)",
                stdout.len(),
                stderr.len(),
            ),
        ));
    }

    server::stop_server();
}

/// Poll for and handle IPC requests.
///
/// Called from the reedline idle callback (~33ms interval).
/// Requests are not processed immediately — they are stored in
/// `PENDING_IPC_OPERATION` and the break signal is fired so that
/// the ExternalBreak handler can check the editor buffer first.
pub fn poll_ipc_requests() {
    // Check if a visible evaluate has completed (R returned to prompt)
    check_visible_eval_completion();

    let receiver = match IPC_RECEIVER.get() {
        Some(r) => r,
        None => return, // IPC not started
    };

    let rx = match receiver.try_lock() {
        Ok(rx) => rx,
        Err(_) => return,
    };

    let rx = match rx.as_ref() {
        Some(rx) => rx,
        None => return, // IPC stopped
    };

    // Process all pending requests
    while let Ok(request) = rx.try_recv() {
        handle_request(request);
    }
}

/// Check if a pending visible evaluate has completed.
///
/// A visible evaluate injects code into the REPL (like user_input) and waits
/// for the result. When R returns to the prompt, the evaluation is done and
/// we can collect captured output and send the reply.
/// Default timeout for visible evaluate (5 minutes).
pub(in crate::ipc) const DEFAULT_EVAL_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(300);

fn check_visible_eval_completion() {
    // Check prompt state for normal completion, and elapsed time for timeout cleanup.
    let at_prompt = r_is_at_prompt().load(Ordering::Acquire);

    let mut guard = match pending_visible_eval().try_lock() {
        Ok(g) => g,
        Err(_) => return,
    };

    // Check for staleness/timeout regardless of prompt state.
    // NOTE: In the timeout path, R may still be actively evaluating the injected code.
    // Calling finish_ipc_capture() here races with R's WriteConsoleEx calls — some output
    // may be lost or partial. This is a best-effort cleanup to avoid indefinite hangs;
    // subsequent R output from the timed-out evaluation will go to the default handler.
    if let Some(pending) = guard.as_ref()
        && std::time::Instant::now() > pending.deadline
    {
        let pending = guard.take().unwrap();
        // Best-effort capture cleanup (may race with active R evaluation)
        let (stdout, stderr) = arf_libr::finish_ipc_capture();
        let _ = pending.reply.send(IpcResponse::error(
            R_BUSY,
            format!(
                "Visible evaluate timed out after {}s (stdout: {} bytes, stderr: {} bytes)",
                pending.timeout.as_secs(),
                stdout.len(),
                stderr.len(),
            ),
        ));
        // Do NOT set r_is_at_prompt(true) here — R may still be evaluating.
        // The flag will be set when R actually returns to the prompt via
        // set_r_at_prompt(true) in the normal REPL flow.
        return;
    }

    if !at_prompt {
        return;
    }

    if let Some(pending) = guard.take() {
        // Prevent new requests from starting during capture finalization
        r_is_at_prompt().store(false, Ordering::Release);

        // Finish WriteConsoleEx capture and collect output
        let (stdout, stderr) = arf_libr::finish_ipc_capture();

        let result = EvaluateResult {
            stdout,
            stderr,
            // In visible mode, auto-printed values are in stdout and errors in stderr.
            // Structured value/error fields are not available because the code runs
            // through normal REPL evaluation (no tryCatch wrapper).
            value: None,
            error: None,
        };

        let _ = pending.reply.send(IpcResponse::Evaluate(result));

        // Restore prompt state now that finalization is complete
        r_is_at_prompt().store(true, Ordering::Release);
    }
}

/// Handle a single IPC request on the main thread.
///
/// Instead of processing requests directly, stores them in `PENDING_IPC_OPERATION`
/// and fires the break signal. The actual processing happens in the ExternalBreak
/// handler in the REPL, which can check the editor buffer for mutual exclusion.
fn handle_request(request: IpcRequest) {
    let IpcRequest { method, reply } = request;

    // Session requests are handled immediately without ExternalBreak,
    // since they are read-only and don't conflict with user input.
    // However, we must not touch R (e.g. via arf_harp::eval_string) unless
    // it is safe: R must be idle, not in alternate mode, and no other IPC
    // operation pending that might race with us.
    if matches!(method, IpcMethod::Session) {
        let r_at_prompt = r_is_at_prompt().load(Ordering::Acquire);
        let has_pending = pending_ipc_operation()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some();
        let in_alt_mode = is_in_alternate_mode();

        let try_r = r_at_prompt && !has_pending && !in_alt_mode;
        let reason = if in_alt_mode {
            "R is in alternate mode (shell, history browser, or help browser)"
        } else if !r_at_prompt {
            "R is busy evaluating another expression"
        } else if has_pending {
            "Another IPC operation is pending"
        } else {
            "" // R info will be collected
        };

        let result = collect_session_result(try_r, reason);
        let _ = reply.send(IpcResponse::Session(Box::new(result)));
        return;
    }

    // Reject if in alternate mode (shell, history browser, help browser).
    // Normally dispatch_request() catches this first, but this covers the
    // race where a request was queued just before alternate mode was entered.
    if is_in_alternate_mode() {
        let _ = reply.send(IpcResponse::error(
            R_NOT_AT_PROMPT,
            "R is not at the command prompt".to_string(),
        ));
        return;
    }

    // Reject if there's already a pending operation waiting for ExternalBreak.
    // Uses INPUT_ALREADY_PENDING (not R_BUSY) so clients can distinguish
    // "R is evaluating" from "another IPC request is queued."
    if pending_ipc_operation()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
    {
        let _ = reply.send(IpcResponse::error(
            INPUT_ALREADY_PENDING,
            "Another IPC operation is pending".to_string(),
        ));
        return;
    }

    match method {
        IpcMethod::Evaluate {
            code,
            visible,
            timeout_ms,
        } => {
            // Check if R is at the prompt (idle)
            if !r_is_at_prompt().load(Ordering::Acquire) {
                let _ = reply.send(IpcResponse::error(R_BUSY, "R is busy".to_string()));
                return;
            }

            let timeout = timeout_ms
                .map(std::time::Duration::from_millis)
                .unwrap_or(DEFAULT_EVAL_TIMEOUT);

            let kind = if visible {
                PendingIpcKind::VisibleEvaluate { reply, timeout }
            } else {
                PendingIpcKind::SilentEvaluate { reply }
            };

            // Store operation and fire break signal
            *pending_ipc_operation()
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(PendingIpcOperation { kind, code });
            fire_break_signal();
        }
        IpcMethod::UserInput { code } => {
            // Check if R is at the prompt
            if !r_is_at_prompt().load(Ordering::Acquire) {
                let _ = reply.send(IpcResponse::error(
                    R_NOT_AT_PROMPT,
                    "R is not at the command prompt".to_string(),
                ));
                return;
            }

            // Store operation and fire break signal
            *pending_ipc_operation()
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(PendingIpcOperation {
                kind: PendingIpcKind::UserInput { reply },
                code,
            });
            fire_break_signal();
        }
        IpcMethod::Session => unreachable!("Session handled above"),
    }
}

/// Fire the break signal to interrupt reedline's `read_line()` loop.
fn fire_break_signal() {
    if let Some(signal) = BREAK_SIGNAL.get() {
        signal.store(true, Ordering::Relaxed);
    }
}

// ── Public API for the REPL ──────────────────────────────────────────────

/// Take the pending IPC operation (if any).
///
/// Called from the REPL's ExternalBreak handler and fast-path to process
/// the stored operation after checking the editor buffer.
pub fn take_pending_ipc_operation() -> Option<PendingIpcOperation> {
    pending_ipc_operation()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
}

/// Reject a pending IPC operation because the user is typing.
///
/// Sends a `USER_IS_TYPING` error response to the IPC client.
/// The current editor buffer is included in the JSON-RPC error `data`
/// field so that callers (e.g. AI agents) can see what the user is
/// typing and decide whether to retry or abort. The `message` field
/// is kept stable for pattern matching.
pub fn reject_operation_user_typing(op: PendingIpcOperation, buffer: &str) {
    let response = IpcResponse::error_with_data(
        USER_IS_TYPING,
        "User is typing in the console".to_string(),
        serde_json::json!({ "buffer": buffer }),
    );
    match op.kind {
        PendingIpcKind::SilentEvaluate { reply }
        | PendingIpcKind::VisibleEvaluate { reply, .. }
        | PendingIpcKind::UserInput { reply } => {
            let _ = reply.send(response);
        }
    }
}

/// Set up a visible evaluate: start capture and store the deferred reply.
///
/// Called from the REPL after the buffer check passes. The reply will be
/// sent later by `check_visible_eval_completion` when R returns to the prompt.
pub fn setup_visible_eval(
    reply: tokio::sync::oneshot::Sender<IpcResponse>,
    timeout: std::time::Duration,
) {
    r_is_at_prompt().store(false, Ordering::Release);

    // Start WriteConsoleEx capture (visible=true → also print to terminal)
    arf_libr::start_ipc_capture(true);

    // Store reply channel — will be consumed by check_visible_eval_completion.
    // The deadline is set from now + timeout, aligning with the server-side
    // oneshot timeout that started slightly earlier in dispatch_request.
    *pending_visible_eval()
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(PendingVisibleEval {
        reply,
        deadline: std::time::Instant::now()
            .checked_add(timeout)
            .unwrap_or_else(|| std::time::Instant::now() + DEFAULT_EVAL_TIMEOUT),
        timeout,
    });
}

/// Run a silent evaluate and send the reply.
///
/// Called from the REPL after the buffer check passes. Runs R code with
/// full output capture (stdout/stderr via WriteConsoleEx, value/error via
/// temp file). The response is sent synchronously before returning.
///
/// **Note:** `evaluate_with_capture` does not support interactive R functions
/// (e.g., `readline()`, `browser()`, `menu()`). If the evaluated code triggers
/// a nested `ReadConsole` callback, R will block waiting for input that never
/// arrives, eventually requiring user intervention from the console.
pub fn run_silent_eval(code: &str, reply: tokio::sync::oneshot::Sender<IpcResponse>) {
    r_is_at_prompt().store(false, Ordering::Release);

    let result = capture::evaluate_with_capture(code, false);

    r_is_at_prompt().store(true, Ordering::Release);
    let _ = reply.send(IpcResponse::Evaluate(result));
}

/// Accept a user_input operation: send the success reply.
pub fn accept_user_input(reply: tokio::sync::oneshot::Sender<IpcResponse>) {
    let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: true }));
}

/// Mark that R is now at the command prompt (idle, ready for input).
pub fn set_r_at_prompt(at_prompt: bool) {
    r_is_at_prompt().store(at_prompt, Ordering::Release);
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

// ── Session info collection ───────────────────────────────────────────────

/// Build arf-side info that is always available (no R needed).
fn arf_session_base(meta: &SessionMeta) -> SessionResult {
    SessionResult {
        arf_version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        socket_path: meta.socket_path.clone(),
        started_at: meta.started_at.clone(),
        log_file: meta.log_file.clone(),
        history_session_id: meta.history_session_id,
        r: None,
        r_unavailable_reason: None,
        hint: None,
    }
}

/// In-memory session metadata, set once at IPC server startup.
/// Avoids re-reading the session file on every `session` request.
#[derive(Clone)]
struct SessionMeta {
    socket_path: String,
    started_at: String,
    log_file: Option<String>,
    history_session_id: Option<i64>,
}

static SESSION_META: OnceLock<Mutex<SessionMeta>> = OnceLock::new();

/// Clear the history session ID from both in-memory cache and on-disk session file.
///
/// Called when history initialization fails, so IPC does not advertise
/// a session ID that has no corresponding history backend.
pub fn clear_history_session_id() {
    // Only rewrite the on-disk session file if we actually had a history session ID.
    // This avoids extra file I/O and noisy logs when the value is already None
    // or when IPC was never started (SESSION_META not initialized).
    if let Some(m) = SESSION_META.get() {
        let mut meta = m.lock().unwrap_or_else(|e| e.into_inner());
        if meta.history_session_id.take().is_some() {
            session::clear_session_history_id(std::process::id());
        }
    }
}

/// Set the history backend for headless mode.
///
/// Once set, `headless_handle_request` will persist evaluated commands
/// (both `evaluate` and `user_input`) to the SQLite history database.
pub fn set_headless_history(history: reedline::SqliteBackedHistory) {
    if HEADLESS_HISTORY.set(Mutex::new(history)).is_err() {
        log::warn!(
            "Headless history backend already initialized; ignoring duplicate set_headless_history call"
        );
    }
}

/// Store the history database path and session ID for IPC queries.
///
/// Called during startup (both REPL and headless) so that the `history`
/// IPC method can open a read-only connection to query history entries.
pub fn set_history_db_info(path: PathBuf, session_id: Option<reedline::HistorySessionId>) {
    if HISTORY_DB_INFO.set((path, session_id)).is_err() {
        log::warn!(
            "History DB info already initialized; ignoring duplicate set_history_db_info call"
        );
    }
}

/// Error type for history query failures, distinguishing parameter
/// validation errors from internal/database errors.
pub(crate) enum HistoryQueryError {
    /// Client provided invalid parameters (maps to INVALID_PARAMS).
    InvalidParams(String),
    /// Internal failure such as database open/query error (maps to INTERNAL_ERROR).
    Internal(String),
}

/// Query the history database and return matching entries.
///
/// Opens a read-only `rusqlite::Connection` for each query to avoid WAL
/// and DDL side effects that `SqliteBackedHistory::with_file` would cause.
/// This prevents conflicts with the main REPL/headless history connection.
pub(crate) fn query_history(params: &HistoryParams) -> Result<HistoryResult, HistoryQueryError> {
    if params.limit < 1 {
        return Err(HistoryQueryError::InvalidParams(format!(
            "limit must be positive, got {}",
            params.limit
        )));
    }

    let (db_path, session_id) = HISTORY_DB_INFO.get().ok_or_else(|| {
        HistoryQueryError::Internal(
            "History is not available (no history database configured)".to_string(),
        )
    })?;

    // When not requesting all sessions, require a valid session_id.
    if !params.all_sessions && session_id.is_none() {
        return Err(HistoryQueryError::Internal(
            "Session ID is not available; use all_sessions=true to query without session scope"
                .to_string(),
        ));
    }

    // Parse --since before opening the DB so validation errors are cheap.
    let since_ms = if let Some(ref since) = params.since {
        Some(
            chrono::DateTime::parse_from_rfc3339(since)
                .map(|dt| dt.with_timezone(&chrono::Utc))
                .or_else(|_| {
                    chrono::NaiveDate::parse_from_str(since, "%Y-%m-%d")
                        .map(|d| d.and_hms_opt(0, 0, 0).unwrap().and_utc())
                })
                .map_err(|_| {
                    HistoryQueryError::InvalidParams(format!(
                        "Invalid 'since' format: {since}. \
                         Use RFC 3339 (e.g. '2026-03-29T00:00:00Z') or date (e.g. '2026-03-29')"
                    ))
                })?
                .timestamp_millis(),
        )
    } else {
        None
    };

    // Open read-only to avoid WAL/DDL conflicts with the main history
    // connection (same pattern as pager/history_browser.rs).
    let db =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| {
                HistoryQueryError::Internal(format!("Failed to open history database: {e}"))
            })?;

    // Build SQL query with optional WHERE clauses.
    let mut sql = String::from(
        "SELECT command_line, start_timestamp, session_id, cwd, exit_status \
         FROM history WHERE 1=1",
    );
    let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

    if !params.all_sessions
        && let Some(sid) = session_id
    {
        sql.push_str(" AND session_id = ?");
        params_vec.push(Box::new(i64::from(*sid)));
    }
    if let Some(ref cwd) = params.cwd {
        sql.push_str(" AND cwd = ?");
        params_vec.push(Box::new(cwd.clone()));
    }
    if let Some(ref grep) = params.grep {
        // Use instr() instead of LIKE to avoid wildcard escaping issues
        // (same approach as reedline's Substring search).
        sql.push_str(" AND instr(command_line, ?) >= 1");
        params_vec.push(Box::new(grep.clone()));
    }
    if let Some(ms) = since_ms {
        sql.push_str(" AND start_timestamp >= ?");
        params_vec.push(Box::new(ms));
    }
    sql.push_str(" ORDER BY id DESC LIMIT ?");
    params_vec.push(Box::new(params.limit));

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| &**p).collect();

    let mut stmt = db
        .prepare(&sql)
        .map_err(|e| HistoryQueryError::Internal(format!("Failed to prepare query: {e}")))?;
    let entries = stmt
        .query_map(param_refs.as_slice(), |row| {
            let ts_ms: Option<i64> = row.get(1)?;
            let timestamp: Option<String> = ts_ms.and_then(|ms| {
                chrono::Utc
                    .timestamp_millis_opt(ms)
                    .single()
                    .map(|t| t.to_rfc3339())
            });
            let sid: Option<i64> = row.get(2)?;
            Ok(HistoryEntry {
                command: row.get(0)?,
                timestamp,
                cwd: row.get(3)?,
                exit_status: row.get(4)?,
                session_id: sid,
            })
        })
        .map_err(|e| HistoryQueryError::Internal(format!("History query failed: {e}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| HistoryQueryError::Internal(format!("Failed to read history row: {e}")))?;

    Ok(HistoryResult {
        entries,
        session_id: session_id.map(i64::from),
    })
}

/// Save a command to the headless history database, if configured.
///
/// Errors are logged but never propagated — history saving must not
/// interfere with IPC response delivery.
fn save_to_headless_history(code: &str, exit_status: Option<i64>) {
    let Some(h) = HEADLESS_HISTORY.get() else {
        return;
    };
    let Ok(mut history) = h.lock() else {
        log::warn!("Headless history lock poisoned, skipping save");
        return;
    };
    use reedline::History;
    let mut item = reedline::HistoryItem::from_command_line(code);
    item.start_timestamp = Some(chrono::Utc::now());
    item.hostname = Some(gethostname::gethostname().to_string_lossy().into_owned());
    item.cwd = std::env::current_dir()
        .ok()
        .map(|p| p.to_string_lossy().into_owned());
    item.exit_status = exit_status;
    item.session_id = HISTORY_DB_INFO.get().and_then(|(_, sid)| *sid);
    if let Err(e) = history.save(item) {
        log::warn!("Failed to save headless history: {}", e);
    }
}

/// Store session metadata in memory (called after server start).
pub(in crate::ipc) fn set_session_meta(
    socket_path: String,
    started_at: String,
    log_file: Option<String>,
    history_session_id: Option<i64>,
) {
    let meta = SessionMeta {
        socket_path,
        started_at,
        log_file,
        history_session_id,
    };
    match SESSION_META.get() {
        Some(m) => *m.lock().unwrap_or_else(|e| e.into_inner()) = meta,
        None => {
            let _ = SESSION_META.set(Mutex::new(meta));
        }
    }
}

/// Get cached session metadata.
///
/// Uses a blocking lock with poison recovery. If `set_session_meta` was never
/// called (which should not happen in practice), returns explicit placeholder
/// strings instead of empty values to avoid emitting ambiguous metadata.
fn current_session_meta() -> SessionMeta {
    match SESSION_META.get() {
        Some(m) => m.lock().unwrap_or_else(|e| e.into_inner()).clone(),
        None => SessionMeta {
            socket_path: "<uninitialized_socket_path>".to_string(),
            started_at: "<uninitialized_started_at>".to_string(),
            log_file: None,
            history_session_id: None,
        },
    }
}

/// Collect session information, including R info if `try_r` is true and R is idle.
///
/// Called from both REPL idle callback and headless mode handler.
/// When R is busy or unavailable, returns arf-only info with an explanation.
///
/// `reason` is used as the `r_unavailable_reason` when `try_r` is false.
/// When `try_r` is true but R is not at the prompt, a default reason is used.
pub(in crate::ipc) fn collect_session_result(try_r: bool, reason: &str) -> SessionResult {
    let meta = current_session_meta();
    let mut result = arf_session_base(&meta);

    if !try_r || !r_is_at_prompt().load(Ordering::Acquire) {
        let reason = if reason.is_empty() {
            "R is busy evaluating another expression"
        } else {
            reason
        };
        result.r_unavailable_reason = Some(reason.to_string());
        result.hint = Some(if reason.contains("alternate mode") {
            "Exit the current mode (shell, browser) to make R session info available.".to_string()
        } else if reason.contains("pending") {
            "Wait for the current IPC operation to complete, then retry.".to_string()
        } else if reason.contains("Main thread") || reason.contains("handler dropped") {
            "The arf process may be shutting down or unresponsive.".to_string()
        } else if reason.contains("Timed out") {
            "R may be busy with a long-running operation. Retry later.".to_string()
        } else {
            "R session information will be available when R returns to the prompt. \
             Retry 'arf ipc session' later, or use 'arf ipc eval' with a timeout to wait."
                .to_string()
        });
        return result;
    }

    match collect_r_session_info() {
        Some(r_info) => {
            result.r = Some(r_info);
        }
        None => {
            result.r_unavailable_reason =
                Some("Failed to collect R session information".to_string());
            result.hint = Some(
                "R may not be fully initialized. Try again later or use \
                 'arf ipc eval \"sessionInfo()\"' for raw output."
                    .to_string(),
            );
        }
    }

    result
}

/// Collect R session information using base R functions.
///
/// Must be called on the main R thread when R is at the prompt.
/// Returns `None` if R is not available or evaluation fails.
///
/// Each piece of information is collected via a separate `eval_string` call
/// and extracted as a raw Rust string/vector. JSON serialization is handled
/// entirely by serde_json, so no manual escaping is needed on the R side.
fn collect_r_session_info() -> Option<RSessionInfo> {
    let version = eval_r_scalar(r#"invisible(paste0(R.version$major, ".", R.version$minor))"#)
        .unwrap_or_default();
    if version.is_empty() {
        return None;
    }

    Some(RSessionInfo {
        version,
        platform: eval_r_scalar("invisible(R.version$platform)").unwrap_or_default(),
        locale: eval_r_scalar("invisible(Sys.getlocale())").unwrap_or_default(),
        cwd: eval_r_scalar("invisible(getwd())").unwrap_or_default(),
        loaded_namespaces: eval_r_character_vector("invisible(loadedNamespaces())")
            .unwrap_or_default(),
        attached_packages: eval_r_character_vector("invisible(.packages())").unwrap_or_default(),
        lib_paths: eval_r_character_vector("invisible(.libPaths())").unwrap_or_default(),
    })
}

/// Evaluate an R expression and extract a single string result.
fn eval_r_scalar(code: &str) -> Option<String> {
    match arf_harp::eval_string(code) {
        Ok(robj) => extract_r_string(robj.sexp()),
        Err(e) => {
            log::debug!("eval_r_scalar failed for `{code}`: {e}");
            None
        }
    }
}

/// Evaluate an R expression and extract a character vector result.
fn eval_r_character_vector(code: &str) -> Option<Vec<String>> {
    match arf_harp::eval_string(code) {
        Ok(robj) => extract_r_strings(robj.sexp()),
        Err(e) => {
            log::debug!("eval_r_character_vector failed for `{code}`: {e}");
            None
        }
    }
}

/// Extract a single string from an R SEXP (character vector of length >= 1).
fn extract_r_string(sexp: arf_libr::SEXP) -> Option<String> {
    let lib = arf_libr::r_library().ok()?;
    unsafe {
        if (lib.rf_isstring)(sexp) == 0 || (lib.rf_length)(sexp) == 0 {
            return None;
        }
        let elt = (lib.string_elt)(sexp, 0);
        let cstr = (lib.r_charsxp)(elt);
        if cstr.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(cstr)
            .to_str()
            .ok()
            .map(|s| s.to_string())
    }
}

/// Extract all strings from an R SEXP character vector.
fn extract_r_strings(sexp: arf_libr::SEXP) -> Option<Vec<String>> {
    let lib = arf_libr::r_library().ok()?;
    unsafe {
        if (lib.rf_isstring)(sexp) == 0 {
            return None;
        }
        let len = (lib.rf_length)(sexp) as isize;
        let mut result = Vec::with_capacity(len as usize);
        for i in 0..len {
            let elt = (lib.string_elt)(sexp, i);
            let cstr = (lib.r_charsxp)(elt);
            if cstr.is_null() {
                result.push(String::new());
            } else if let Ok(s) = std::ffi::CStr::from_ptr(cstr).to_str() {
                result.push(s.to_string());
            } else {
                result.push(String::new());
            }
        }
        Some(result)
    }
}

// ── Headless mode API ────────────────────────────────────────────────────

/// Poll and directly process IPC requests in headless mode.
///
/// Unlike `poll_ipc_requests` (used by the REPL), this function processes
/// requests immediately without the ExternalBreak/editor-buffer check,
/// since there is no interactive editor in headless mode.
///
/// Returns `true` if at least one request was processed.
pub fn headless_poll_and_process() -> bool {
    let receiver = match IPC_RECEIVER.get() {
        Some(r) => r,
        None => return false,
    };

    let rx = match receiver.try_lock() {
        Ok(rx) => rx,
        Err(_) => return false,
    };

    let rx = match rx.as_ref() {
        Some(rx) => rx,
        None => return false,
    };

    let mut processed = false;

    while let Ok(request) = rx.try_recv() {
        processed = true;
        headless_handle_request(request);
    }

    processed
}

/// Handle a single IPC request in headless mode.
///
/// Processes evaluate and user_input requests directly on the R thread.
/// No editor buffer check is needed since there is no interactive console.
fn headless_handle_request(request: IpcRequest) {
    let IpcRequest { method, reply } = request;

    match method {
        IpcMethod::Evaluate { code, visible, .. } => {
            // Note: timeout_ms is intentionally ignored here. In headless mode,
            // evaluate_with_capture() runs synchronously on the main thread, so
            // R evaluation cannot be interrupted. The server-side oneshot timeout
            // (in dispatch_request) still applies as a backstop.
            //
            // When visible=true, captured output is also written to the
            // headless process's stdout/stderr for logging/monitoring.
            r_is_at_prompt().store(false, Ordering::Release);
            let result = capture::evaluate_with_capture(&code, visible);
            r_is_at_prompt().store(true, Ordering::Release);

            // Determine exit status before moving result into the reply.
            let has_error = result.error.is_some();
            let _ = reply.send(IpcResponse::Evaluate(result));

            // Save after reply so SQLite I/O doesn't delay the IPC response.
            if !code.trim().is_empty() {
                let exit_status = if has_error { 1 } else { 0 };
                save_to_headless_history(&code, Some(exit_status));
            }
        }
        IpcMethod::UserInput { code } => {
            // In headless mode, user_input evaluates the code directly.
            // Output goes to the default WriteConsoleEx handler (stdout/stderr).
            r_is_at_prompt().store(false, Ordering::Release);
            let eval_result = arf_harp::eval_string(&code);
            r_is_at_prompt().store(true, Ordering::Release);
            let exit_status;
            match eval_result {
                Ok(_) => {
                    exit_status = 0;
                    let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: true }));
                }
                Err(e) => {
                    exit_status = 1;
                    log::warn!("Headless user_input evaluation error: {}", e);
                    let _ = reply.send(IpcResponse::UserInput(UserInputResult { accepted: false }));
                }
            }

            if !code.trim().is_empty() {
                save_to_headless_history(&code, Some(exit_status));
            }
        }
        IpcMethod::Session => {
            let result = collect_session_result(true, "");
            let _ = reply.send(IpcResponse::Session(Box::new(result)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Drop guard that resets global IPC state on scope exit (including panics).
    struct GlobalStateGuard;

    impl Drop for GlobalStateGuard {
        fn drop(&mut self) {
            set_in_alternate_mode(false);
            set_r_at_prompt(false);
        }
    }

    /// Tests for the IN_ALTERNATE_MODE flag and handle_request rejection.
    ///
    /// Serialized with `#[serial]` because all tests that touch the global
    /// `IN_ALTERNATE_MODE` / `R_IS_AT_PROMPT` atomics must not run concurrently.
    #[test]
    #[serial]
    fn test_alternate_mode_flag_and_request_rejection() {
        // Reset global state and ensure cleanup on panic via Drop guard
        set_in_alternate_mode(false);
        set_r_at_prompt(false);
        let _guard = GlobalStateGuard;

        assert!(!is_in_alternate_mode());

        // Toggle on/off
        set_in_alternate_mode(true);
        assert!(is_in_alternate_mode());
        set_in_alternate_mode(false);
        assert!(!is_in_alternate_mode());

        // Set R at prompt so we test alternate mode rejection specifically
        // (not the R_BUSY / R_NOT_AT_PROMPT check that comes after)
        set_r_at_prompt(true);
        set_in_alternate_mode(true);
        {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let request = IpcRequest {
                method: IpcMethod::UserInput {
                    code: "1+1".to_string(),
                },
                reply: reply_tx,
            };
            handle_request(request);
            match reply_rx.blocking_recv().unwrap() {
                IpcResponse::Error { code, .. } => assert_eq!(code, R_NOT_AT_PROMPT),
                _ => panic!("Expected R_NOT_AT_PROMPT error for user_input"),
            }
        }

        // handle_request rejects evaluate in alternate mode
        {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let request = IpcRequest {
                method: IpcMethod::Evaluate {
                    code: "1+1".to_string(),
                    visible: false,
                    timeout_ms: None,
                },
                reply: reply_tx,
            };
            handle_request(request);
            match reply_rx.blocking_recv().unwrap() {
                IpcResponse::Error { code, .. } => assert_eq!(code, R_NOT_AT_PROMPT),
                _ => panic!("Expected R_NOT_AT_PROMPT error for evaluate"),
            }
        }

        // Cleanup handled by GlobalStateGuard drop
    }

    /// Tests that `handle_request` returns arf-only session info (not an error)
    /// in various states: alternate mode, R busy, pending operation.
    #[test]
    #[serial]
    fn test_session_returns_arf_only_in_various_states() {
        set_in_alternate_mode(false);
        set_r_at_prompt(false);
        let _guard = GlobalStateGuard;

        // Helper: send a Session request and get the result
        fn send_session() -> protocol::SessionResult {
            let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
            let request = IpcRequest {
                method: IpcMethod::Session,
                reply: reply_tx,
            };
            handle_request(request);
            match reply_rx.blocking_recv().unwrap() {
                IpcResponse::Session(result) => *result,
                _ => panic!("Expected Session response"),
            }
        }

        // Case 1: alternate mode — should return arf-only with alternate mode reason
        set_in_alternate_mode(true);
        set_r_at_prompt(true);
        {
            let result = send_session();
            assert!(result.r.is_none());
            let reason = result.r_unavailable_reason.unwrap();
            assert!(
                reason.contains("alternate mode"),
                "Expected alternate mode reason, got: {reason}"
            );
        }

        // Case 2: R busy (not at prompt) — should return arf-only
        set_in_alternate_mode(false);
        set_r_at_prompt(false);
        {
            let result = send_session();
            assert!(result.r.is_none());
            assert!(result.r_unavailable_reason.is_some());
        }

        // Case 3: pending operation — should return arf-only
        set_r_at_prompt(true);
        {
            // Insert a dummy pending operation
            let (dummy_tx, _dummy_rx) = tokio::sync::oneshot::channel();
            *pending_ipc_operation()
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = Some(PendingIpcOperation {
                kind: PendingIpcKind::SilentEvaluate { reply: dummy_tx },
                code: "dummy".to_string(),
            });

            let result = send_session();
            assert!(result.r.is_none());
            let reason = result.r_unavailable_reason.unwrap();
            assert!(
                reason.contains("pending"),
                "Expected pending reason, got: {reason}"
            );

            // Clean up dummy pending operation
            let _ = take_pending_ipc_operation();
        }
    }
}
