//! Session metadata management for IPC discovery.
//!
//! Each arf process with IPC enabled writes a session file to
//! `~/.cache/arf/sessions/<pid>.json` so that clients can discover
//! running sessions and their socket paths.
//!
//! Set `ARF_IPC_SESSIONS_DIR` to override the sessions directory for both
//! writers and readers. This is useful for hermetic tests and explicit
//! multi-instance isolation.
//!
//! # `ARF_IPC_SESSIONS_DIR` usage notes
//!
//! - **Use an absolute path.** Relative paths are resolved against each
//!   process's current working directory; if the writer and reader start
//!   from different directories they will silently look in different
//!   locations.
//! - **Use a user-private directory.** The default (`~/.cache/arf/sessions`)
//!   is created with mode 0700. When overriding to an already-existing
//!   directory (e.g. `/tmp/my-dir`), ensure it is not world-accessible;
//!   although session files are created with mode 0600, their filenames
//!   (which reveal PIDs) would be visible to other users.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const ARF_IPC_SESSIONS_DIR: &str = "ARF_IPC_SESSIONS_DIR";

/// Session metadata written to disk for client discovery.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub pid: u32,
    pub socket_path: String,
    pub r_version: Option<String>,
    pub cwd: String,
    pub started_at: String,
    /// Log file path, or `None` if no log file is configured.
    #[serde(default)]
    pub log_file: Option<String>,
    /// History session ID (nanosecond timestamp), or `None` when history is
    /// disabled, when no history directory is available, or when the history
    /// database failed to open.
    #[serde(default)]
    pub history_session_id: Option<i64>,
}

/// Return the directory where session files are stored.
pub fn sessions_dir() -> Option<PathBuf> {
    if let Some(override_dir) = std::env::var_os(ARF_IPC_SESSIONS_DIR) {
        let path = PathBuf::from(override_dir);
        if !path.as_os_str().is_empty() {
            return Some(path);
        }
    }
    dirs::cache_dir().map(|d| d.join("arf").join("sessions"))
}

/// Write session metadata to disk.
///
/// On Unix, the sessions directory is created with mode 0700 and the session
/// file with mode 0600 so that other users cannot discover or connect to the
/// IPC socket.
pub fn write_session(info: &SessionInfo) -> std::io::Result<()> {
    let dir = sessions_dir().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "cache directory not found")
    })?;
    // Create directory with mode 0700 atomically on Unix to avoid TOCTOU
    // race between create_dir_all and set_permissions.
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&dir)?;
    }

    let path = dir.join(format!("{}.json", info.pid));
    let json = serde_json::to_string_pretty(info).map_err(std::io::Error::other)?;

    // On Unix, create the file with restricted permissions atomically
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(json.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, &json)?;
    }

    log::info!("Session file written: {}", path.display());
    Ok(())
}

/// Clear the `history_session_id` field in the on-disk session file for this process.
///
/// Reads the current session file, sets `history_session_id` to `null`, and rewrites it.
/// Errors are logged but not propagated since this is a best-effort cleanup.
pub fn clear_session_history_id(pid: u32) {
    let Some(dir) = sessions_dir() else { return };
    let path = dir.join(format!("{pid}.json"));
    let contents = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            log::debug!("Could not read session file {}: {}", path.display(), e);
            return;
        }
    };
    let mut info: SessionInfo = match serde_json::from_str(&contents) {
        Ok(i) => i,
        Err(e) => {
            log::debug!("Could not parse session file {}: {}", path.display(), e);
            return;
        }
    };
    info.history_session_id = None;
    // Ensure we rewrite the same file even if the stored PID differs
    // (e.g. due to file corruption or tampering).
    info.pid = pid;
    if let Err(e) = write_session(&info) {
        log::debug!("Could not rewrite session file {}: {}", path.display(), e);
    }
}

/// Remove session metadata on shutdown.
pub fn remove_session(pid: u32) {
    if let Some(dir) = sessions_dir() {
        let path = dir.join(format!("{pid}.json"));
        if let Err(e) = std::fs::remove_file(&path) {
            log::debug!("Could not remove session file {}: {}", path.display(), e);
        }
    }
}

/// List all session files, filtering out stale ones (where the process no longer exists).
pub fn list_sessions() -> Vec<SessionInfo> {
    let dir = match sessions_dir() {
        Some(d) if d.exists() => d,
        _ => return Vec::new(),
    };

    let mut sessions = Vec::new();
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json")
            && let Ok(contents) = std::fs::read_to_string(&path)
            && let Ok(info) = serde_json::from_str::<SessionInfo>(&contents)
        {
            if is_process_alive(info.pid) {
                sessions.push(info);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
    }

    sessions
}

/// Find a session by PID, or return the only running session if PID is not specified.
pub fn find_session(pid: Option<u32>) -> Option<SessionInfo> {
    let sessions = list_sessions();
    match pid {
        Some(target_pid) => sessions.into_iter().find(|s| s.pid == target_pid),
        None => {
            if sessions.len() == 1 {
                sessions.into_iter().next()
            } else {
                None
            }
        }
    }
}

/// Check if a process with the given PID is still running.
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // On Unix, sending signal 0 checks process existence without actually signaling.
        // Returns 0 on success, -1 on error. EPERM means the process exists but we
        // lack permission to signal it — still alive.
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if ret == 0 {
            return true;
        }
        // EPERM: process exists but not owned by us
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(windows)]
    {
        use std::process::Command;
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .is_ok_and(|o| {
                let out = String::from_utf8_lossy(&o.stdout);
                // CSV format: "name","pid",...  — match exact PID field
                out.lines().any(|line| {
                    line.split(',')
                        .nth(1)
                        .is_some_and(|f| f.trim_matches('"') == pid.to_string())
                })
            })
    }
}
