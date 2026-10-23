//! Construction of the `session` result: cached arf-side metadata plus R
//! session information collected from the live R process.

use super::policy;
use super::protocol::{RSessionInfo, SessionResult};
use super::session;
use super::state::r_is_at_prompt;
use crate::sync::LockRecover;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::sync::{Mutex, OnceLock, atomic::Ordering};

/// In-memory session metadata, set once at IPC server startup.
/// Avoids re-reading the session file on every `session` request.
#[derive(Clone)]
struct SessionMeta {
    socket_path: String,
    started_at: String,
    r_home: Option<String>,
    log_file: Option<String>,
    history_session_id: Option<i64>,
    session_type: session::SessionType,
}

static SESSION_META: OnceLock<Mutex<SessionMeta>> = OnceLock::new();

/// The random identifier of this process, as reported in `session_id`.
///
/// Built from two hashes seeded by the standard library's per-process random
/// keys, mixed with the pid and the current time, so it is not guessable from
/// the pid alone and differs between runs.
pub(super) fn session_id() -> &'static str {
    static SESSION_ID: OnceLock<String> = OnceLock::new();
    SESSION_ID.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let part = || {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u32(std::process::id());
            hasher.write_u128(nanos);
            hasher.finish()
        };
        format!("{:016x}{:016x}", part(), part())
    })
}

/// Build arf-side info that is always available (no R needed).
fn arf_session_base(meta: &SessionMeta) -> SessionResult {
    SessionResult {
        arf_version: env!("CARGO_PKG_VERSION").to_string(),
        pid: std::process::id(),
        session_id: session_id().to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        r_home: meta.r_home.clone(),
        socket_path: meta.socket_path.clone(),
        started_at: meta.started_at.clone(),
        log_file: meta.log_file.clone(),
        history_session_id: meta.history_session_id,
        ipc_policy: policy::policy(meta.session_type),
        r: None,
        r_unavailable_reason: None,
        hint: None,
    }
}

/// Clear the history session ID from both in-memory cache and on-disk session file.
///
/// Called when history initialization fails, so IPC does not advertise
/// a session ID that has no corresponding history backend.
pub fn clear_history_session_id() {
    // Only rewrite the on-disk session file if we actually had a history session ID.
    // This avoids extra file I/O and noisy logs when the value is already None
    // or when IPC was never started (SESSION_META not initialized).
    if let Some(m) = SESSION_META.get() {
        let mut meta = m.lock_recover();
        if meta.history_session_id.take().is_some() {
            session::clear_session_history_id(std::process::id());
        }
    }
}

/// Store session metadata in memory (called after server start).
pub(super) fn set_session_meta(
    socket_path: String,
    started_at: String,
    r_home: Option<String>,
    log_file: Option<String>,
    history_session_id: Option<i64>,
    session_type: session::SessionType,
) {
    let meta = SessionMeta {
        socket_path,
        started_at,
        r_home,
        log_file,
        history_session_id,
        session_type,
    };
    match SESSION_META.get() {
        Some(m) => *m.lock_recover() = meta,
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
        Some(m) => m.lock_recover().clone(),
        None => SessionMeta {
            socket_path: "<uninitialized_socket_path>".to_string(),
            started_at: "<uninitialized_started_at>".to_string(),
            r_home: None,
            log_file: None,
            history_session_id: None,
            session_type: session::SessionType::Interactive,
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
pub(super) fn collect_session_result(try_r: bool, reason: &str) -> SessionResult {
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

/// Read the running R version (for example `"4.4.1"`) for the session file.
///
/// Must be called on the main R thread. R writes the version to a temporary
/// file because `eval_string` does not hand back a value. Returns `None` if
/// the temporary file cannot be created or read.
pub fn read_r_version() -> Option<String> {
    let tmpfile = tempfile::Builder::new()
        .prefix(".arf_ipc_rver_")
        .suffix(".txt")
        .tempfile()
        .ok()?;
    let tmppath = tmpfile.path().display().to_string().replace('\\', "/");
    let code = format!(r#"writeLines(paste0(R.version$major, ".", R.version$minor), "{tmppath}")"#);
    let _ = arf_harp::eval_string(&code);
    std::fs::read_to_string(tmpfile.path())
        .ok()
        .map(|s| s.trim().to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Tests that the session metadata passed to `set_session_meta` is reflected
    /// in `SessionResult`, both as typed fields and in the JSON representation.
    // Protects the process-global `SESSION_META` session-metadata cache.
    #[test]
    #[serial]
    fn test_session_result_reflects_session_meta() {
        // Every optional field set.
        let session_id: i64 = 1_700_000_000_000_000_000;
        set_session_meta(
            "/tmp/test.sock".to_string(),
            "2026-01-01T00:00:00+00:00".to_string(),
            Some("/opt/R/4.4.1/lib/R".to_string()),
            Some("/tmp/arf.log".to_string()),
            Some(session_id),
            session::SessionType::Interactive,
        );
        let result = collect_session_result(false, "test");
        assert_eq!(result.log_file.as_deref(), Some("/tmp/arf.log"));
        assert_eq!(result.r_home.as_deref(), Some("/opt/R/4.4.1/lib/R"));
        assert_eq!(result.history_session_id, Some(session_id));
        assert_eq!(
            result.ipc_policy.silent,
            crate::ipc::policy::SilentPolicy::Restricted {
                allowed_functions: Vec::new()
            }
        );

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["r_home"], "/opt/R/4.4.1/lib/R");
        assert_eq!(json["log_file"], "/tmp/arf.log");
        assert_eq!(json["history_session_id"], session_id);
        assert_eq!(json["ipc_policy"]["silent"]["mode"], "restricted");

        // Every optional field unset: the JSON fields are still present, as null.
        set_session_meta(
            "/tmp/test.sock".to_string(),
            "2026-01-01T00:00:00+00:00".to_string(),
            None,
            None,
            None,
            session::SessionType::Interactive,
        );
        let result = collect_session_result(false, "test");
        assert_eq!(result.log_file, None);
        assert_eq!(result.r_home, None);
        assert_eq!(result.history_session_id, None);

        let json = serde_json::to_value(&result).unwrap();
        for field in ["log_file", "r_home", "history_session_id"] {
            assert!(
                json.get(field).is_some(),
                "{field} field should always be present in JSON"
            );
            assert!(json[field].is_null(), "{field} should be null when not set");
        }
    }

    #[test]
    fn test_session_id_is_a_stable_random_token() {
        let id = session_id();
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()), "{id}");
        assert_eq!(id, session_id());

        let json = serde_json::to_value(collect_session_result(false, "test")).unwrap();
        assert_eq!(json["session_id"], id);
    }
}
