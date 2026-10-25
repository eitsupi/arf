//! Socket / named-pipe path selection and stale-socket handling.

/// Get the socket/pipe path for a given PID.
///
/// On Unix, uses `$XDG_RUNTIME_DIR/arf/<pid>.sock` (the XDG-correct location
/// for runtime sockets).  Falls back to `<temp_dir>/arf-<random>/<pid>.sock`
/// when `XDG_RUNTIME_DIR` is not set or its directory fails safety validation.
///
/// The socket directory is validated for safety (not a symlink, owned by
/// the current user, not writable by group/other).
/// Returns `(socket_path, dir_created)` where `dir_created` is `true` when
/// the socket directory was freshly created by this call (and should be
/// cleaned up on shutdown).
pub(super) fn get_socket_path(pid: u32) -> Option<(String, bool)> {
    #[cfg(unix)]
    {
        let temp_fallback = || {
            let suffix = random_hex_suffix();
            std::env::temp_dir().join(format!("arf-{suffix}"))
        };
        let mut candidates = Vec::with_capacity(2);
        if let Some(runtime_dir) = dirs::runtime_dir() {
            candidates.push(runtime_dir.join("arf"));
        }
        candidates.push(temp_fallback());
        select_socket_dir(pid, &candidates)
    }
    #[cfg(windows)]
    {
        Some((format!(r"\\.\pipe\arf-ipc-{pid}"), false))
    }
}

/// Generate a short random hex string for use in directory names.
///
/// Uses `HashMap`'s `RandomState` (seeded from platform randomness in
/// the standard library) to avoid adding a `rand` dependency.  The
/// result is 16 hex characters, which is sufficient to make directory
/// names unpredictable in practice.
#[cfg(unix)]
fn random_hex_suffix() -> String {
    use std::hash::{BuildHasher, Hasher};
    let state = std::collections::hash_map::RandomState::new();
    let hash = state.build_hasher().finish();
    format!("{hash:016x}")
}

/// Validate that a directory is safe to use for an IPC socket: not a
/// symlink, is a directory, owned by the current user, and accessible only
/// by the owner (mode `0700`).  Returns `true` if the path does not exist
/// yet (it will be created securely by the caller).
#[cfg(unix)]
fn is_dir_safe(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    match dir.symlink_metadata() {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => {
            log::warn!("Cannot stat socket directory {}: {e}", dir.display());
            false
        }
        Ok(meta) => {
            if meta.file_type().is_symlink() {
                log::warn!(
                    "Socket directory {} is a symlink — refusing to use it",
                    dir.display()
                );
                return false;
            }
            if !meta.is_dir() {
                log::warn!(
                    "Socket directory path {} exists but is not a directory",
                    dir.display()
                );
                return false;
            }
            if meta.uid() != unsafe { libc::getuid() } {
                log::warn!(
                    "Socket directory {} is not owned by the current user",
                    dir.display()
                );
                return false;
            }
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o700 {
                log::warn!(
                    "Socket directory {} must have permissions 0700, found {:o}",
                    dir.display(),
                    mode
                );
                return false;
            }
            true
        }
    }
}

/// Try each candidate directory in order, returning the socket path and
/// whether the directory was created by this call.  Creates the chosen
/// directory with mode `0700` if it does not exist.
#[cfg(unix)]
fn select_socket_dir(pid: u32, candidates: &[std::path::PathBuf]) -> Option<(String, bool)> {
    use std::os::unix::fs::DirBuilderExt;

    for dir in candidates {
        if is_dir_safe(dir) {
            // Attempt a non-recursive mkdir to atomically determine
            // whether we created the directory.  Parent directories
            // (e.g. $XDG_RUNTIME_DIR, $TMPDIR) are expected to exist.
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            let created = match builder.create(dir) {
                Ok(()) => true,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
                Err(e) => {
                    log::warn!("Failed to create directory {}: {e}", dir.display());
                    continue;
                }
            };
            // Re-validate after creation to close the TOCTOU window: if
            // another process created the directory between our initial
            // check and DirBuilder::create, it may have different
            // ownership or permissions.
            if !is_dir_safe(dir) {
                log::warn!(
                    "Socket directory {} failed safety validation after creation",
                    dir.display()
                );
                continue;
            }
            let path = dir.join(format!("{pid}.sock")).display().to_string();
            return Some((path, created));
        }
    }

    let dirs: Vec<_> = candidates.iter().map(|d| d.display().to_string()).collect();
    log::error!(
        "All socket directory candidates failed (unsafe or could not be created): {}. \
         Refusing to start IPC server.",
        dirs.join(", ")
    );
    None
}

/// Remove a stale socket file at `socket_path`, if one exists.
///
/// When a custom `--ipc-bind` path is used (`custom_bind`), only remove the
/// path if it is actually a Unix socket to avoid accidentally deleting
/// unrelated files. For sockets, attempt a connect to distinguish stale from
/// active: if connect succeeds, another process is listening and we must not
/// take over.
#[cfg(unix)]
pub(super) fn probe_stale_socket(socket_path: &str, custom_bind: bool) -> std::io::Result<()> {
    use std::os::unix::net::UnixStream;

    probe_stale_socket_with(
        socket_path,
        custom_bind,
        std::fs::symlink_metadata(socket_path),
        |path| UnixStream::connect(path).map(drop),
    )
}

#[cfg(unix)]
fn probe_stale_socket_with(
    socket_path: &str,
    custom_bind: bool,
    metadata: std::io::Result<std::fs::Metadata>,
    connect: impl FnOnce(&str) -> std::io::Result<()>,
) -> std::io::Result<()> {
    use super::{SocketIdentity, remove_owned_socket};
    use std::os::unix::fs::FileTypeExt;

    match metadata {
        Ok(meta) if meta.file_type().is_socket() => {
            if custom_bind {
                // Custom bind path: verify the socket is stale before removing
                match connect(socket_path) {
                    Ok(_) => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::AlreadyExists,
                            format!("IPC socket already in use at path: {}", socket_path),
                        ));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                        // Only remove the socket observed before connecting.
                        // The final identity check and unlink are not atomic.
                        remove_owned_socket(
                            socket_path,
                            Some(SocketIdentity::from_metadata(&meta)),
                        );
                    }
                    // The socket disappeared. Do not unlink a later replacement.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(std::io::Error::new(
                            e.kind(),
                            format!("Cannot probe socket at {}: {}", socket_path, e),
                        ));
                    }
                }
            } else {
                // Retain PID-reuse cleanup, but preserve a replacement socket.
                remove_owned_socket(socket_path, Some(SocketIdentity::from_metadata(&meta)));
            }
        }
        Ok(_) if custom_bind => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!(
                    "bind path already exists and is not a socket: {}",
                    socket_path
                ),
            ));
        }
        Ok(meta) => {
            // Preserve cleanup of non-sockets at default PID-based paths,
            // but only while the original entry still occupies the path.
            // As with socket cleanup, this check and unlink are not atomic.
            if let Ok(current) = std::fs::symlink_metadata(socket_path)
                && SocketIdentity::from_metadata(&current) == SocketIdentity::from_metadata(&meta)
            {
                let _ = std::fs::remove_file(socket_path);
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // Does not exist
        Err(e) => return Err(e), // Propagate unexpected errors (e.g. EACCES)
    }
    Ok(())
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::{is_dir_safe, random_hex_suffix, select_socket_dir};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn probe_preserves_socket_replaced_after_metadata() {
        use std::io::{Error, ErrorKind};
        use std::os::unix::net::{UnixListener, UnixStream};

        for (custom_bind, connect_error, original_is_socket) in [
            (true, ErrorKind::NotFound, true),
            (true, ErrorKind::ConnectionRefused, true),
            (false, ErrorKind::ConnectionRefused, true),
            (false, ErrorKind::ConnectionRefused, false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("probe.sock");
            // Keep the original inode alive so replacement cannot reuse it.
            let _original_socket = original_is_socket.then(|| UnixListener::bind(&path).unwrap());
            let _original_file =
                (!original_is_socket).then(|| std::fs::File::create(&path).unwrap());
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            let _replacement = UnixListener::bind(&path).unwrap();

            super::probe_stale_socket_with(
                path.to_str().unwrap(),
                custom_bind,
                Ok(metadata),
                |_| Err(Error::from(connect_error)),
            )
            .unwrap();

            UnixStream::connect(&path).unwrap_or_else(|error| {
                panic!("replacement lost (custom={custom_bind}, {connect_error:?}): {error}")
            });
        }
    }

    #[test]
    fn random_hex_suffix_has_expected_format() {
        let suffix = random_hex_suffix();
        assert_eq!(suffix.len(), 16);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn nonexistent_dir_is_safe() {
        let tmp = tempfile::tempdir().unwrap();
        let candidate = tmp.path().join("does-not-exist");
        assert!(is_dir_safe(&candidate));
    }

    #[test]
    fn dir_with_0700_is_safe() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("good");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(is_dir_safe(&dir));
    }

    #[test]
    fn dir_with_group_read_is_unsafe() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("leaky");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(!is_dir_safe(&dir));
    }

    #[test]
    fn dir_with_other_write_is_unsafe() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("world");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o702)).unwrap();
        assert!(!is_dir_safe(&dir));
    }

    #[test]
    fn symlink_is_unsafe() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("real");
        std::fs::create_dir(&target).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(!is_dir_safe(&link));
    }

    #[test]
    fn regular_file_is_unsafe() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-dir");
        std::fs::write(&file, "").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!is_dir_safe(&file));
    }

    #[test]
    fn select_uses_existing_safe_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o700)).unwrap();
        let fallback = tmp.path().join("fallback");
        // First candidate already exists and is safe — should be selected.
        let (path, created) =
            select_socket_dir(12345, &[existing.clone(), fallback.clone()]).unwrap();
        assert!(path.contains("existing"));
        assert!(
            !created,
            "dir already existed, should not report as created"
        );
        assert!(!fallback.exists(), "fallback should not have been created");
    }

    #[test]
    fn select_uses_first_safe_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let good = tmp.path().join("good");
        let also_good = tmp.path().join("also-good");
        // Neither exists yet — both are safe, first should win.
        let (path, created) = select_socket_dir(12345, &[good.clone(), also_good.clone()]).unwrap();
        assert!(path.contains("good"));
        assert!(created, "dir did not exist, should report as created");
        assert!(good.exists(), "first candidate should have been created");
        assert!(
            !also_good.exists(),
            "second candidate should not have been created"
        );
    }

    #[test]
    fn select_skips_unsafe_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("bad");
        std::fs::create_dir(&bad).unwrap();
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o777)).unwrap();
        let good = tmp.path().join("fallback");
        let (path, created) = select_socket_dir(12345, &[bad, good.clone()]).unwrap();
        assert!(path.contains("fallback"));
        assert!(created, "fallback dir should have been created");
    }

    #[test]
    fn select_returns_none_when_all_unsafe() {
        let tmp = tempfile::tempdir().unwrap();
        let bad1 = tmp.path().join("bad1");
        std::fs::create_dir(&bad1).unwrap();
        std::fs::set_permissions(&bad1, std::fs::Permissions::from_mode(0o777)).unwrap();
        let bad2 = tmp.path().join("bad2");
        std::fs::write(&bad2, "").unwrap(); // regular file, not a dir
        std::fs::set_permissions(&bad2, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = select_socket_dir(12345, &[bad1, bad2]);
        assert!(result.is_none());
    }
}
