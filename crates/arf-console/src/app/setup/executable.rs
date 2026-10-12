//! Locating and checking R executables shared by environment providers.

use std::path::Path;

/// File name of the R launcher on the current platform.
#[cfg(windows)]
pub(super) const R_EXECUTABLE: &str = "R.exe";
/// File name of the R launcher on the current platform.
#[cfg(not(windows))]
pub(super) const R_EXECUTABLE: &str = "R";

/// Whether `path` is a regular file that can be executed.
///
/// On non-Unix platforms any regular file qualifies.
pub(super) fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}
