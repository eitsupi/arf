//! Session metadata management for IPC discovery.
//!
//! Each arf process with IPC enabled writes a session file to
//! `~/.cache/arf/sessions/<pid>.json` so that clients can discover
//! running sessions and their socket paths.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Session metadata written to disk for client discovery.
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    pub pid: u32,
    pub socket_path: String,
    pub r_version: Option<String>,
    pub cwd: String,
    pub started_at: String,
}

/// Return the directory where session files are stored.
pub fn sessions_dir() -> Option<PathBuf> {
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
