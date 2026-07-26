//! PID file management for `--ipc-pid-file` / `arf headless --pid-file`.

use anyhow::{Context, Result};

/// Write the current process ID to a file.
///
/// The file is created with restricted permissions (0600 on Unix) and is
/// intended to be removed on shutdown by the caller.
pub(crate) fn write_pid_file(path: &std::path::Path) -> Result<()> {
    let pid = std::process::id().to_string();
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

pub(crate) fn absolute_pid_file_path(path: &std::path::Path) -> std::path::PathBuf {
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

    // Disarm the atexit handler only after attempting the same absolute path.
    IPC_PID_FILE_CLEANUP_DONE.store(true, std::sync::atomic::Ordering::Release);
}
