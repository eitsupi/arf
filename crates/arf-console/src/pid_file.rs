//! PID file management for `--ipc-pid-file` / `arf headless --pid-file`.

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Environment carriers used only for a restart replacement.  The path is a
/// separate value so Unix paths that are not valid UTF-8 are preserved.
pub(crate) const RESTART_PID_ENV: &str = "_ARF_INTERNAL_RESTART_PID";
pub(crate) const RESTART_PID_PATH_ENV: &str = "_ARF_INTERNAL_RESTART_PID_PATH";

#[derive(Clone, Debug)]
pub(crate) struct RestartPidContext {
    pub(crate) pid: u32,
    pub(crate) path: PathBuf,
}

static RESTART_PID_CONTEXT: std::sync::OnceLock<RestartPidContext> = std::sync::OnceLock::new();
static INITIAL_PID_FILE_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Consume the restart carriers before R or any child process can observe
/// them. Invalid or incomplete carriers are ignored and cannot enable adopt.
pub(crate) fn capture_restart_context() {
    // SAFETY: called during single-threaded process startup, before R and its
    // threads are initialized.
    let (pid, path) = unsafe {
        let pid = std::env::var_os(RESTART_PID_ENV);
        let path = std::env::var_os(RESTART_PID_PATH_ENV);
        std::env::remove_var(RESTART_PID_ENV);
        std::env::remove_var(RESTART_PID_PATH_ENV);
        (pid, path)
    };

    let Some(pid) = pid.and_then(|value| value.to_str()?.parse::<u32>().ok()) else {
        return;
    };
    let Some(path) = path.map(PathBuf::from).filter(|path| path.is_absolute()) else {
        return;
    };
    let _ = RESTART_PID_CONTEXT.set(RestartPidContext { pid, path });
}

pub(crate) fn restart_pid_context() -> Option<RestartPidContext> {
    RESTART_PID_CONTEXT.get().cloned()
}

/// Resolve the configured path before R profiles can change the working
/// directory. A restart carrier takes precedence so relative CLI paths keep
/// referring to the original process's absolute path.
pub(crate) fn set_initial_pid_file_path(path: &Path) {
    let resolved = restart_pid_context()
        .map(|context| context.path)
        .unwrap_or_else(|| absolute_path(path));
    let _ = INITIAL_PID_FILE_PATH.set(resolved);
}

pub(crate) fn initial_pid_file_path() -> Option<PathBuf> {
    INITIAL_PID_FILE_PATH.get().cloned()
}

/// Return the carriers needed by a replacement command, if this session has
/// a PID file. OsString is intentional: Unix path arguments may be non-UTF-8.
pub(crate) fn restart_command_context() -> Option<(OsString, OsString)> {
    let path = initial_pid_file_path()?;
    Some((path.into_os_string(), std::process::id().to_string().into()))
}

pub(crate) fn restart_context_carrier() -> Option<(OsString, OsString)> {
    let context = restart_pid_context()?;
    Some((
        context.path.into_os_string(),
        context.pid.to_string().into(),
    ))
}

/// Write the current process ID to a file.
///
/// The file is created with restricted permissions (0600 on Unix) and is
/// intended to be removed on shutdown by the caller.
pub(crate) fn write_pid_file(path: &std::path::Path) -> Result<()> {
    let pid = std::process::id().to_string();

    #[cfg(unix)]
    if let Some(context) = restart_pid_context() {
        // Unix exec keeps the process ID and the old file open only in the
        // filesystem namespace. Adopt only the exact file/path/PID tuple; in
        // particular, symlinks and directories are never adopted.
        let matches = context.pid == std::process::id()
            && context.path == path
            && pid_file_contains_current_pid(path, pid.as_bytes());
        if matches {
            log::info!("Adopted PID file for restart: {}", path.display());
            return Ok(());
        }
    }

    // Use create_new to fail if the file already exists, avoiding overwrite
    // of unrelated files or symlink-following attacks.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("Failed to create PID file: {}", path.display()))?;
        file.write_all(pid.as_bytes())
            .with_context(|| format!("Failed to write PID file: {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        // create_new on Windows also fails if the file exists
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .and_then(|mut f| {
                use std::io::Write;
                f.write_all(pid.as_bytes())
            })
            .with_context(|| format!("Failed to create PID file: {}", path.display()))?;
    }
    log::info!("PID file written: {}", path.display());
    Ok(())
}

/// Check the existing PID file through one descriptor. `O_NOFOLLOW` and
/// descriptor metadata prevent a symlink replacement between the type check
/// and the read from being treated as an adoptable file.
#[cfg(unix)]
fn pid_file_contains_current_pid(path: &Path, expected: &[u8]) -> bool {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    let Ok(file) = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    else {
        return false;
    };
    let Ok(metadata) = file.metadata() else {
        return false;
    };
    if !metadata.file_type().is_file() {
        return false;
    }

    // Read at most one byte beyond the expected value. This both verifies
    // exact contents and avoids allocating for an unexpectedly large file.
    let mut contents = Vec::with_capacity(expected.len() + 1);
    if file
        .take(expected.len() as u64 + 1)
        .read_to_end(&mut contents)
        .is_err()
    {
        return false;
    }
    contents == expected
}

pub(crate) fn absolute_pid_file_path(path: &std::path::Path) -> std::path::PathBuf {
    INITIAL_PID_FILE_PATH
        .get()
        .cloned()
        .unwrap_or_else(|| absolute_path(path))
}

fn absolute_path(path: &Path) -> PathBuf {
    std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Absolute path of the PID file written by `--ipc-pid-file`.
///
/// Stored as an absolute path so that the `atexit` handler can remove it even
/// if the process changes its working directory before exiting.  R's
/// `q()` calls `exit()` without running Rust destructors, so cleanup of the
/// PID file is registered here as an `atexit` handler rather than relying on
/// the normal code path after `repl.run()` / `run_headless()`.
static IPC_PID_FILE_PATH: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Set to `true` by the normal cleanup path (REPL or headless) after it has
/// removed the PID file, so the `atexit` handler does not race to delete a
/// replacement file created by a subsequent process at the same path.
static IPC_PID_FILE_CLEANUP_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Register an `atexit` handler that removes the IPC PID file on process exit.
///
/// Called once after `write_pid_file` succeeds, for both `--with-ipc` (REPL)
/// and `arf headless` modes.  The handler is a safety net for paths where R
/// calls `exit()` directly (e.g. `q()` or EOF) and bypasses Rust cleanup code.
pub(crate) fn register_ipc_pid_file_atexit(path: &std::path::Path) {
    // Convert to absolute path so the handler works regardless of cwd changes.
    let _ = IPC_PID_FILE_PATH.set(absolute_pid_file_path(path));

    let ret = unsafe { libc::atexit(remove_ipc_pid_file_at_exit) };
    if ret != 0 {
        log::warn!("Failed to register IPC PID file cleanup with atexit");
    }
}

extern "C" fn remove_ipc_pid_file_at_exit() {
    // Skip if the normal cleanup path already removed the file to avoid
    // unlinking a replacement file created by a subsequent process.
    if IPC_PID_FILE_CLEANUP_DONE.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    if let Some(path) = IPC_PID_FILE_PATH.get() {
        let _ = std::fs::remove_file(path);
    }
}

pub(crate) fn cleanup_ipc_pid_file(path: &std::path::Path) {
    let cleanup_path = IPC_PID_FILE_PATH
        .get()
        .cloned()
        .unwrap_or_else(|| absolute_pid_file_path(path));

    if let Err(e) = std::fs::remove_file(&cleanup_path) {
        log::debug!(
            "Could not remove PID file {}: {}",
            cleanup_path.display(),
            e
        );
    }

    // Disarm after attempting the same absolute path. This is the normal
    // final cleanup path; restart handoff uses the fallible relinquish helper
    // below and disarms only after successful removal.
    IPC_PID_FILE_CLEANUP_DONE.store(true, std::sync::atomic::Ordering::Release);
}

/// Remove the parent's PID file before a non-Unix replacement is spawned.
/// The atexit handler remains armed if removal fails, so the old PID is never
/// restored or overwritten by a best-effort operation.
#[cfg(not(unix))]
pub(crate) fn relinquish_pid_file_for_restart(path: &Path) -> Result<()> {
    let cleanup_path = IPC_PID_FILE_PATH
        .get()
        .cloned()
        .unwrap_or_else(|| absolute_pid_file_path(path));
    std::fs::remove_file(&cleanup_path).with_context(|| {
        format!(
            "Failed to relinquish PID file before restart: {}",
            cleanup_path.display()
        )
    })?;
    IPC_PID_FILE_CLEANUP_DONE.store(true, std::sync::atomic::Ordering::Release);
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::pid_file_contains_current_pid;

    #[test]
    fn adoption_check_requires_exact_regular_file_contents() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let path = directory.path().join("arf.pid");
        std::fs::write(&path, b"1234").expect("write PID file");

        assert!(pid_file_contains_current_pid(&path, b"1234"));
        assert!(!pid_file_contains_current_pid(&path, b"123"));
        std::fs::write(&path, b"1234\n").expect("write non-exact PID file");
        assert!(!pid_file_contains_current_pid(&path, b"1234"));
    }

    #[test]
    fn adoption_check_rejects_symlink() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let target = directory.path().join("target");
        let link = directory.path().join("arf.pid");
        std::fs::write(&target, b"1234").expect("write target file");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");

        assert!(!pid_file_contains_current_pid(&link, b"1234"));
    }
}
