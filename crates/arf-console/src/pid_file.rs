//! PID file management for `--ipc-pid-file` / `arf headless --pid-file`.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// The only restart capability carried through the environment is the
/// inherited PID-file descriptor number. The authoritative path comes from
/// the normalized argv, never from an environment value.
#[cfg(unix)]
pub(crate) const RESTART_PID_FD_ENV: &str = "_ARF_INTERNAL_RESTART_PID_FD";

#[cfg(unix)]
static INHERITED_PID_FD: std::sync::OnceLock<Option<std::os::unix::io::RawFd>> =
    std::sync::OnceLock::new();
#[cfg(unix)]
static INHERITED_PID_FD_VALID: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
#[cfg(unix)]
static OWNED_PID_FD: std::sync::OnceLock<std::sync::Mutex<Option<std::fs::File>>> =
    std::sync::OnceLock::new();
static INITIAL_PID_FILE_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Consume the restart fd carrier before R or any child process can observe
/// it. Parsing does not acquire or close the descriptor.
pub(crate) fn capture_restart_context() {
    #[cfg(unix)]
    {
        // SAFETY: called during single-threaded process startup, before R and
        // its threads are initialized.
        let fd = unsafe {
            let value = std::env::var_os(RESTART_PID_FD_ENV);
            std::env::remove_var(RESTART_PID_FD_ENV);
            value
                .and_then(|value| value.to_str()?.parse::<std::os::unix::io::RawFd>().ok())
                .filter(|fd| *fd >= 3)
        };

        let _ = INHERITED_PID_FD.set(fd);
    }
}

/// Resolve the configured path before R profiles can change the working
/// directory.
pub(crate) fn set_initial_pid_file_path(path: &Path) {
    let resolved = absolute_path(path);
    let _ = INITIAL_PID_FILE_PATH.set(resolved);
}

#[cfg(not(unix))]
pub(crate) fn initial_pid_file_path() -> Option<PathBuf> {
    INITIAL_PID_FILE_PATH.get().cloned()
}

/// Validate the inherited descriptor against the authoritative CLI path before
/// allowing it to cross another loader re-exec.
#[cfg(unix)]
pub(crate) fn authorize_inherited_pid_fd(path: &Path) {
    let valid = INHERITED_PID_FD.get().copied().flatten().is_some_and(|fd| {
        validate_inherited_pid_fd(fd, path, std::process::id().to_string().as_bytes())
    });
    let _ = INHERITED_PID_FD_VALID.set(valid);
}

#[cfg(unix)]
fn owned_pid_fd() -> &'static std::sync::Mutex<Option<std::fs::File>> {
    OWNED_PID_FD.get_or_init(|| std::sync::Mutex::new(None))
}

#[cfg(unix)]
pub(crate) fn restart_fd_carrier() -> Option<std::ffi::OsString> {
    use std::os::unix::io::AsRawFd;
    if INHERITED_PID_FD_VALID.get().copied() == Some(true) {
        let fd = INHERITED_PID_FD.get().copied().flatten()?;
        return Some(fd.to_string().into());
    }
    let guard = owned_pid_fd().lock().ok()?;
    guard
        .as_ref()
        .map(|file| file.as_raw_fd().to_string().into())
}

#[cfg(unix)]
pub(crate) fn finish_loader_reexec() {
    if INHERITED_PID_FD_VALID.get().copied() == Some(true)
        && let Some(fd) = INHERITED_PID_FD.get().copied().flatten()
    {
        let _ = set_fd_cloexec(fd, true);
    }
}

/// Write the current process ID to a file.
///
/// The file is created with restricted permissions (0600 on Unix) and is
/// intended to be removed on shutdown by the caller.
pub(crate) fn write_pid_file(path: &std::path::Path) -> Result<()> {
    let pid = std::process::id().to_string();

    #[cfg(unix)]
    if let Some(fd) = INHERITED_PID_FD.get().copied().flatten() {
        use std::os::unix::io::FromRawFd;
        // The path is authoritative from argv. Adopt only when the inherited
        // descriptor and path resolve to the same regular file and its exact
        // contents identify this process.
        if validate_inherited_pid_fd(fd, path, pid.as_bytes()) {
            // SAFETY: validation succeeded and the descriptor was supplied by
            // the prior arf image. Ownership is transferred exactly once.
            let file = unsafe { std::fs::File::from_raw_fd(fd) };
            *owned_pid_fd().lock().unwrap() = Some(file);
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
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("Failed to create PID file: {}", path.display()))?;
        file.write_all(pid.as_bytes())
            .with_context(|| format!("Failed to write PID file: {}", path.display()))?;
        *owned_pid_fd().lock().unwrap() = Some(move_file_to_safe_fd(file)?);
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
#[cfg(test)]
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

#[cfg(unix)]
fn validate_inherited_pid_fd(fd: std::os::unix::io::RawFd, path: &Path, expected: &[u8]) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    // Never close the carrier fd during validation. A clone is used for all
    // reads, while the original remains available for adoption or is left
    // untouched when validation fails.
    let borrowed = unsafe { std::os::unix::io::BorrowedFd::borrow_raw(fd) };
    let file = match borrowed.try_clone_to_owned() {
        Ok(file) => std::fs::File::from(file),
        Err(_) => return false,
    };
    let fd_meta = match file.metadata() {
        Ok(meta) if meta.file_type().is_file() => meta,
        _ => return false,
    };
    let path_file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(_) => return false,
    };
    let path_meta = match path_file.metadata() {
        Ok(meta) if meta.file_type().is_file() => meta,
        _ => return false,
    };
    if fd_meta.dev() != path_meta.dev() || fd_meta.ino() != path_meta.ino() {
        return false;
    }

    let mut contents = Vec::with_capacity(expected.len() + 1);
    let mut reader = file;
    if reader.seek(SeekFrom::Start(0)).is_err()
        || reader
            .take(expected.len() as u64 + 1)
            .read_to_end(&mut contents)
            .is_err()
    {
        return false;
    }
    contents == expected
}

/// Temporarily make the owned PID file descriptor survive Unix exec. The
/// guard restores FD_CLOEXEC if exec returns with an error.
#[cfg(unix)]
pub(crate) struct PidFdExecGuard {
    fd: std::os::unix::io::RawFd,
}

#[cfg(unix)]
impl Drop for PidFdExecGuard {
    fn drop(&mut self) {
        let _ = set_fd_cloexec(self.fd, true);
    }
}

#[cfg(unix)]
pub(crate) fn prepare_pid_fd_for_exec() -> Option<anyhow::Result<PidFdExecGuard>> {
    use std::os::unix::io::AsRawFd;
    let guard = owned_pid_fd().lock().ok()?;
    let fd = guard.as_ref()?.as_raw_fd();
    Some(set_fd_cloexec(fd, false).map(|()| PidFdExecGuard { fd }))
}

#[cfg(unix)]
fn set_fd_cloexec(fd: std::os::unix::io::RawFd, enabled: bool) -> anyhow::Result<()> {
    let current = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if current < 0 {
        return Err(anyhow::anyhow!(std::io::Error::last_os_error()));
    }
    let flags = if enabled {
        current | libc::FD_CLOEXEC
    } else {
        current & !libc::FD_CLOEXEC
    };
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags) } < 0 {
        return Err(anyhow::anyhow!(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(unix)]
fn move_file_to_safe_fd(file: std::fs::File) -> anyhow::Result<std::fs::File> {
    use std::os::unix::io::{FromRawFd, IntoRawFd};
    let raw = file.into_raw_fd();
    let safe = unsafe { libc::fcntl(raw, libc::F_DUPFD_CLOEXEC, 3) };
    if safe < 0 {
        // This function owns the raw descriptor after into_raw_fd(); close it
        // explicitly when duplication fails.
        unsafe { libc::close(raw) };
        return Err(anyhow::anyhow!(std::io::Error::last_os_error()));
    }
    unsafe { libc::close(raw) };
    Ok(unsafe { std::fs::File::from_raw_fd(safe) })
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
    use super::{pid_file_contains_current_pid, validate_inherited_pid_fd};
    use std::os::unix::io::AsRawFd;

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

    #[test]
    fn inherited_fd_validation_requires_matching_inode_and_content() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let path = directory.path().join("arf.pid");
        let other = directory.path().join("other.pid");
        std::fs::write(&path, b"1234").expect("write PID file");
        std::fs::write(&other, b"1234").expect("write other file");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .expect("open PID file");
        let fd = file.as_raw_fd();

        assert!(validate_inherited_pid_fd(fd, &path, b"1234"));
        assert!(!validate_inherited_pid_fd(fd, &other, b"1234"));
        assert!(!validate_inherited_pid_fd(fd, &path, b"5678"));
    }

    #[test]
    fn inherited_fd_validation_rejects_symlink_and_invalid_fd_without_closing() {
        let directory = tempfile::tempdir().expect("create temp directory");
        let target = directory.path().join("target");
        let link = directory.path().join("arf.pid");
        std::fs::write(&target, b"1234").expect("write target");
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");

        let file = std::fs::File::open(&target).expect("open target");
        let fd = file.as_raw_fd();
        assert!(!validate_inherited_pid_fd(fd, &link, b"1234"));
        assert!(!validate_inherited_pid_fd(999_999, &target, b"1234"));
        // The valid descriptor remains usable after validation failures.
        assert_eq!(file.metadata().expect("descriptor remains open").len(), 4);
    }
}
