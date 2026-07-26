//! Logger initialization and stderr redirection.

/// Initialize logging.
///
/// When `log_file` is `Some`, log output is written to the specified file
/// instead of stderr. This is useful for daemon deployments where stderr
/// may not be monitored.
///
/// When `redirect_stderr` is `true` and a log file is provided, the process's
/// stderr file descriptor is also redirected to the log file via `dup2`. This
/// ensures that *all* stderr output — including `eprintln!()` calls, R's
/// `WriteConsoleEx` default output (e.g., from graphics device callbacks), and
/// any other code writing directly to fd 2 — goes to the log file instead of
/// the terminal.
pub(crate) fn init_logger(log_file: Option<&std::path::Path>, redirect_stderr: bool) {
    let mut builder = env_logger::Builder::from_default_env();
    if let Some(path) = log_file {
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        // Restrict log file permissions on Unix (logs may contain sensitive data)
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
            // Prevent following symlinks when opening the log file to avoid
            // appending logs to an unintended target via a symlink.
            opts.custom_flags(libc::O_NOFOLLOW);
        }
        match opts.open(path) {
            Ok(file) => {
                // Ensure restricted permissions even if the file already existed
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let perms = std::fs::Permissions::from_mode(0o600);
                    // Use fd-based set_permissions (fchmod) to avoid TOCTOU
                    // symlink race with path-based std::fs::set_permissions.
                    if let Err(e) = file.set_permissions(perms) {
                        eprintln!(
                            "Warning: could not set permissions on log file {}: {e}",
                            path.display()
                        );
                    }
                }

                // Redirect process stderr to the log file so that all output
                // (not just log::* macros) is captured. This borrows `file`
                // before it is moved into env_logger, but the dup2'd fd is
                // independent of the original.
                if redirect_stderr {
                    redirect_stderr_to_file(&file);
                }

                builder.target(env_logger::Target::Pipe(Box::new(file)));
            }
            Err(e) => {
                eprintln!("Warning: could not open log file {}: {e}", path.display());
                eprintln!("         Falling back to stderr.");
            }
        }
    }
    builder.init();
}

/// Redirect the process's stderr file descriptor to the given file.
///
/// Uses `dup2` to make fd 2 (stderr) point to the same file description as
/// the provided file. After this call, `eprintln!()`, R's `WriteConsoleEx`
/// default output path, and any other code writing to stderr will write to
/// the file instead of the terminal.
#[cfg(unix)]
fn redirect_stderr_to_file(file: &std::fs::File) {
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    // Safety: dup2 is safe with valid file descriptors.
    let ret = unsafe { libc::dup2(fd, libc::STDERR_FILENO) };
    if ret == -1 {
        // Use eprintln! because this runs before the logger is initialized
        // (builder.init() hasn't been called yet). If dup2 failed, stderr
        // is still connected to the original terminal, so eprintln! works.
        eprintln!(
            "Warning: failed to redirect stderr to log file: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// Redirect the C runtime's stderr fd (fd 2) to the given file.
///
/// The Win32 `STD_ERROR_HANDLE` is left unchanged; only the CRT fd used by
/// `eprintln!()` and similar Rust macros is redirected. Uses `DuplicateHandle`
/// to create an independent OS handle before handing it to the CRT.
#[cfg(windows)]
fn redirect_stderr_to_file(file: &std::fs::File) {
    use std::os::windows::io::AsRawHandle;

    // Duplicate the OS handle so the C runtime and the `File` object own
    // independent handles. Without this, `_open_osfhandle` transfers ownership
    // to the C runtime while `File` retains the same value, causing a
    // double-close when both are dropped.
    let mut dup_handle: windows_sys::Win32::Foundation::HANDLE = std::ptr::null_mut();
    let cur_proc = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcess() };
    let ok = unsafe {
        windows_sys::Win32::Foundation::DuplicateHandle(
            cur_proc,
            file.as_raw_handle() as _,
            cur_proc,
            &mut dup_handle,
            0,
            0, // not inheritable
            windows_sys::Win32::Foundation::DUPLICATE_SAME_ACCESS,
        )
    };
    if ok == 0 {
        eprintln!(
            "Warning: failed to duplicate handle for stderr redirect: {}",
            std::io::Error::last_os_error()
        );
        return;
    }

    // Convert the duplicated OS handle to a C runtime fd.
    // Use O_WRONLY | O_APPEND to match the append-mode log file; omitting an
    // explicit access mode can leave fd 2 effectively read-only on some CRTs,
    // causing CRT writes to stderr to fail.
    // MSVC CRT: _O_WRONLY = 0x0001, _O_APPEND = 0x0008
    const O_WRONLY: libc::c_int = 0x0001;
    let new_fd =
        unsafe { libc::open_osfhandle(dup_handle as libc::intptr_t, O_WRONLY | libc::O_APPEND) };
    if new_fd == -1 {
        eprintln!("Warning: failed to convert handle for stderr redirect");
        // Clean up the duplicated handle since open_osfhandle failed.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(dup_handle);
        }
        return;
    }

    // Redirect C runtime's fd 2 (stderr) to the new fd.
    if unsafe { libc::dup2(new_fd, 2) } == -1 {
        eprintln!(
            "Warning: failed to redirect stderr to log file: {}",
            std::io::Error::last_os_error()
        );
    }

    // Close new_fd — dup2 gave fd 2 its own reference to the underlying
    // handle, so new_fd is no longer needed.
    unsafe {
        libc::close(new_fd);
    }
}
