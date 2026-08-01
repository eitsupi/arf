use crate::error::{RError, RResult};
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

static R_AUTO_DISCOVERY_DISABLED: AtomicBool = AtomicBool::new(false);

/// Extract R_HOME from the output of `R RHOME`.
///
/// R's wrapper script prints its warnings to stdout rather than stderr, so
/// the path can be preceded by lines such as `WARNING: ignoring environment
/// value of R_HOME`. Take the last non-empty line, which is the path itself.
///
/// Warning lines are skipped explicitly as well, so output carrying no path
/// at all yields `None` instead of a warning masquerading as one. That check
/// only matches R's untranslated wording; the last-line rule is what makes
/// this correct in general.
pub fn r_home_from_rhome_output(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("WARNING:"))
        .map(str::to_owned)
}

/// Set whether automatic R discovery is disabled for this process.
pub fn set_r_auto_discovery_disabled(disabled: bool) {
    R_AUTO_DISCOVERY_DISABLED.store(disabled, Ordering::Relaxed);
}

/// Return whether automatic R discovery is disabled for this process.
pub fn is_r_auto_discovery_disabled() -> bool {
    R_AUTO_DISCOVERY_DISABLED.load(Ordering::Relaxed)
}

/// Default R library paths by platform.
#[cfg(target_os = "linux")]
const R_LIB_PATHS: &[&str] = &[
    "/opt/R/current/lib/R/lib/libR.so",
    "/usr/lib/R/lib/libR.so",
    "/usr/lib64/R/lib/libR.so",
    "/usr/local/lib/R/lib/libR.so",
];

#[cfg(target_os = "macos")]
const R_LIB_PATHS: &[&str] = &[
    "/Library/Frameworks/R.framework/Versions/Current/Resources/lib/libR.dylib",
    "/opt/R/arm64/lib/R/lib/libR.dylib",
    "/usr/local/lib/R/lib/libR.dylib",
];

/// Default R library paths for Windows.
/// On Windows, R installation paths vary widely, so we rely primarily on
/// R_HOME environment variable or finding R in PATH.
#[cfg(target_os = "windows")]
const R_LIB_PATHS: &[&str] = &[];

/// Get the R shared library folder relative to R_HOME for each platform.
#[cfg(unix)]
fn r_lib_folder() -> &'static str {
    "lib"
}

#[cfg(windows)]
fn r_lib_folder() -> &'static str {
    // On Windows x64, R.dll is in bin/x64/
    // On Windows ARM64, R.dll is in bin/
    #[cfg(target_arch = "aarch64")]
    {
        "bin"
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        "bin\\x64"
    }
}

/// Find the R shared library path.
pub fn find_r_library() -> RResult<PathBuf> {
    // First, check R_HOME environment variable
    if let Ok(r_home) = env::var("R_HOME") {
        let lib_path = r_library_path(Path::new(&r_home));
        if lib_path.exists() {
            return Ok(lib_path);
        }
    }

    if is_r_auto_discovery_disabled() {
        return Err(RError::LibraryNotFound(no_r_auto_discovery_message()));
    }

    // Try to get R_HOME from R itself
    #[cfg(unix)]
    let r_cmd = "R";
    #[cfg(windows)]
    let r_cmd = "R.exe";

    if let Ok(output) = Command::new(r_cmd).args(["RHOME"]).output()
        && output.status.success()
        && let Some(r_home) = r_home_from_rhome_output(&String::from_utf8_lossy(&output.stdout))
    {
        let lib_path = r_library_path(Path::new(&r_home));
        if lib_path.exists() {
            return Ok(lib_path);
        }
    }

    // Try default paths
    for path in R_LIB_PATHS {
        let path = PathBuf::from(path);
        if path.exists() {
            return Ok(path);
        }
    }

    Err(RError::LibraryNotFound(
        "Could not find R library. Please set R_HOME or ensure R is in PATH.".to_string(),
    ))
}

fn no_r_auto_discovery_message() -> String {
    "R automatic discovery is disabled. Set R_HOME, pass --r-home, or remove --no-r-auto-discovery."
        .to_string()
}

/// Get the R library filename for the current platform.
#[cfg(target_os = "linux")]
fn r_lib_name() -> &'static str {
    "libR.so"
}

#[cfg(target_os = "macos")]
fn r_lib_name() -> &'static str {
    "libR.dylib"
}

#[cfg(target_os = "windows")]
fn r_lib_name() -> &'static str {
    "R.dll"
}

/// Return the R shared library path relative to an R_HOME directory.
pub fn r_library_path(r_home: &Path) -> PathBuf {
    r_home.join(r_lib_folder()).join(r_lib_name())
}

/// Return the R_HOME directory for a shared library path returned by
/// [`r_library_path`].
pub fn r_home_from_library_path(library_path: &Path) -> Option<PathBuf> {
    if library_path.file_name()?.to_str()? != r_lib_name() {
        return None;
    }

    let mut r_home = library_path;
    for _ in 0..=Path::new(r_lib_folder()).components().count() {
        r_home = r_home.parent()?;
    }
    Some(r_home.to_path_buf())
}

/// Get R_HOME from the system.
pub fn get_r_home() -> RResult<PathBuf> {
    // Check environment variable first
    if let Ok(r_home) = env::var("R_HOME") {
        return Ok(PathBuf::from(r_home));
    }

    if is_r_auto_discovery_disabled() {
        return Err(RError::LibraryNotFound(no_r_auto_discovery_message()));
    }

    // Try to get from R command
    let output = Command::new("R")
        .args(["RHOME"])
        .output()
        .map_err(|e| RError::LibraryNotFound(format!("Failed to run R RHOME: {}", e)))?;

    if output.status.success() {
        r_home_from_rhome_output(&String::from_utf8_lossy(&output.stdout))
            .map(PathBuf::from)
            .ok_or_else(|| {
                RError::LibraryNotFound(
                    "R RHOME succeeded but printed no usable path. Is R installed correctly?"
                        .to_string(),
                )
            })
    } else {
        Err(RError::LibraryNotFound(
            "R RHOME failed. Is R installed and in PATH?".to_string(),
        ))
    }
}

/// Environment variables that R's shell wrapper script exports but that are
/// absent from `$R_HOME/etc/Renviron`. When embedding R we bypass the wrapper,
/// so these must be extracted from the script and set manually.
const R_WRAPPER_ENV_VARS: &[&str] = &["R_DOC_DIR", "R_SHARE_DIR", "R_INCLUDE_DIR"];

/// Parse R's shell wrapper script (`$R_HOME/bin/R`) to extract environment
/// variable assignments for paths that are not set via `Renviron`.
///
/// The wrapper is generated from R's `src/scripts/R.sh.in` template and
/// contains lines like:
/// ```text
/// R_DOC_DIR=/usr/share/doc/R
/// export R_DOC_DIR
/// ```
///
/// On most installations `$R_HOME/doc` etc. exist and match these values, but
/// some distributions (e.g. Fedora, RHEL) relocate them. Without these
/// variables, `R.home("doc")` falls back to the non-existent `$R_HOME/doc`.
///
/// Note: ark solves the same problem by spawning
/// `R --vanilla -s -e "cat(R.home('share'), ...)"` to query the values.
///
/// We parse the wrapper script directly instead to avoid the ~300ms R
/// startup cost, which matters for a terminal application.
pub(super) fn set_r_path_vars_from_wrapper(r_home: &Path) {
    let wrapper_path = r_home.join("bin").join("R");
    let content = match std::fs::read_to_string(&wrapper_path) {
        Ok(c) => c,
        Err(e) => {
            log::debug!(
                "Could not read R wrapper script {}: {}",
                wrapper_path.display(),
                e
            );
            return;
        }
    };

    for var_name in R_WRAPPER_ENV_VARS {
        // Skip if already set in the environment
        if env::var(var_name).is_ok() {
            continue;
        }

        if let Some(value) = parse_var_from_wrapper_script(&content, var_name) {
            log::debug!("Setting {} from R wrapper: {}", var_name, value);
            // SAFETY: We're in single-threaded initialization
            unsafe { env::set_var(var_name, &value) };
        }
    }
}

/// Extract a variable assignment from an R wrapper script.
///
/// Looks for lines of the form `VAR_NAME=value` and returns the value with
/// surrounding quotes stripped. Returns `None` if the variable is not found
/// or the value is empty.
pub(super) fn parse_var_from_wrapper_script(
    script_content: &str,
    var_name: &str,
) -> Option<String> {
    let value = script_content.lines().find_map(|line| {
        let trimmed = line.trim();
        // Split on first '=' and compare the key exactly to avoid partial
        // prefix matches (e.g. "R_DOC_DIR_EXTRA=" matching "R_DOC_DIR").
        let (key, val) = trimmed.split_once('=')?;
        if key == var_name { Some(val) } else { None }
    })?;

    // Strip surrounding quotes if present
    let value = value.trim_matches('\'').trim_matches('"');

    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Check if LD_LIBRARY_PATH includes the R library directory.
/// If not, re-execute the current process with the correct LD_LIBRARY_PATH.
///
/// This is necessary because LD_LIBRARY_PATH must be set before the process
/// starts for R packages to find libR.so when loading their shared libraries.
///
/// Returns Ok(true) if re-exec happened (caller should exit),
/// Ok(false) if no re-exec needed.
#[cfg(unix)]
pub fn ensure_ld_library_path() -> RResult<bool> {
    ensure_ld_library_path_with_pre_exec(|| {})
}

#[cfg(unix)]
pub fn ensure_ld_library_path_with_pre_exec<F>(pre_exec: F) -> RResult<bool>
where
    F: FnOnce(),
{
    let lib_path = find_r_library()?;
    let Some(lib_dir) = lib_path.parent() else {
        return Ok(false);
    };

    let lib_dir_str = lib_dir.to_string_lossy();
    let current = env::var("LD_LIBRARY_PATH").unwrap_or_default();

    // Check if lib_dir is already in LD_LIBRARY_PATH
    if current.split(':').any(|p| p == lib_dir_str) {
        return Ok(false);
    }

    // Need to re-exec with updated LD_LIBRARY_PATH
    let new_path = if current.is_empty() {
        lib_dir_str.to_string()
    } else {
        format!("{}:{}", lib_dir_str, current)
    };

    // SAFETY: We're about to exec, so modifying environment is safe
    unsafe { env::set_var("LD_LIBRARY_PATH", &new_path) };

    // Re-execute the current process. Preserve OsString arguments so non-UTF-8
    // paths (for example --ipc-pid-file) do not panic during re-exec.
    let args: Vec<_> = env::args_os().skip(1).collect();
    let exe = env::current_exe().map_err(|e| RError::LibraryNotFound(e.to_string()))?;

    log::info!("Re-executing with LD_LIBRARY_PATH={}", new_path);

    pre_exec();

    use std::os::unix::process::CommandExt;
    let err = Command::new(&exe).args(args).exec();
    Err(RError::LibraryNotFound(format!(
        "Failed to re-exec: {}",
        err
    )))
}

#[cfg(not(unix))]
pub fn ensure_ld_library_path() -> RResult<bool> {
    Ok(false)
}
