//! R profile startup file loading.
//!
//! This module handles loading site-level and user-level R profile files
//! (.Rprofile) at startup.
//!
//! Based on ark's startup module:
//! <https://github.com/posit-dev/ark/blob/ca75dbb466875c8d3cd04ad8fbf5684d59b31ba1/crates/ark/src/startup.rs>
//!
//! # Why Manual Loading?
//!
//! We source profiles manually using `sys.source()` wrapped in `R_ToplevelExec()`
//! for the following reasons:
//!
//! 1. **`globalCallingHandlers()` compatibility**: Sourcing in a context wrapped
//!    with `withCallingHandlers()` prevents `.Rprofile` code from calling
//!    `globalCallingHandlers()`. This is commonly used in packages like
//!    Gabor's `prompt` package.
//!
//! 2. **Error handling**: We can catch and report errors that occur during
//!    profile loading.
//!
//! 3. **Control**: Loading profiles after full initialization gives us better
//!    control over the startup sequence.

use std::ffi::CString;
use std::path::{Path, PathBuf};

use arf_libr::{RLibrary, SEXP, SexpType, r_library, r_nil_value};

use crate::error::{HarpError, HarpResult};
use crate::protect::RProtect;

/// Check if site R profile loading should be skipped based on command line args.
pub fn should_ignore_site_r_profile(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--no-site-file" || arg == "--vanilla")
}

/// Check if user R profile loading should be skipped based on command line args.
pub fn should_ignore_user_r_profile(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--no-init-file" || arg == "--vanilla")
}

/// Source the site-level R profile (Rprofile.site).
///
/// Search order:
/// 1. `R_PROFILE` environment variable
/// 2. `$R_HOME/etc/{arch}/Rprofile.site` (arch-specific, typically Windows)
/// 3. `$R_HOME/etc/Rprofile.site`
pub fn source_site_r_profile(r_home: &Path) {
    let Some(path) = find_site_r_profile(r_home) else {
        log::trace!("No site R profile found");
        return;
    };
    source_r_profile(&path);
}

/// Source the user-level R profile (.Rprofile).
///
/// Search order:
/// 1. `R_PROFILE_USER` environment variable
/// 2. `./.Rprofile` (current directory)
/// 3. `~/.Rprofile` (user home directory)
pub fn source_user_r_profile() {
    let Some(path) = find_user_r_profile() else {
        log::trace!("No user R profile found");
        return;
    };
    source_r_profile(&path);
}

/// Source an R profile file.
///
/// Uses `sys.source(file, envir = .GlobalEnv)` wrapped in `R_ToplevelExec()`
/// for safe error handling and to avoid issues with `globalCallingHandlers()`.
fn source_r_profile(path: &Path) {
    let path_str = path.to_string_lossy().to_string();

    log::info!("Found R profile at '{path_str}', sourcing now");

    if !path.exists() {
        log::warn!("R profile at '{path_str}' does not exist, skipping source");
        return;
    }

    match source_r_profile_impl(&path_str) {
        Ok(()) => {
            log::info!("Successfully sourced R profile at '{path_str}'");
        }
        Err(err) => {
            // Log the error and print to stderr so the user sees it
            log::error!("Error while sourcing R profile at '{path_str}': {err}");
            eprintln!("Error while sourcing R profile file at path '{path_str}':\n{err}");
        }
    }
}

/// Internal implementation of profile sourcing using R's sys.source().
fn source_r_profile_impl(path: &str) -> HarpResult<()> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        // Build the call: sys.source(path, .GlobalEnv)
        //
        // sys.source signature: sys.source(file, envir = parent.frame(), ...)
        // We use positional arguments: sys.source(path, .GlobalEnv)

        let path_cstring = CString::new(path).map_err(|_| HarpError::TypeMismatch {
            expected: "valid path".to_string(),
            actual: "path with null byte".to_string(),
        })?;

        // Create the path string
        let path_sexp = protect.protect((lib.rf_mkstring)(path_cstring.as_ptr()));

        // Get sys.source symbol
        let sys_source_sym = install_symbol("sys.source")?;

        // Get .GlobalEnv
        let global_env = *lib.r_globalenv;

        // Build the call with positional arguments:
        // sys.source(path, .GlobalEnv)
        //
        // In R's internal representation (LANGSXP):
        // (sys.source path global_env)
        let nil = r_nil_value()?;

        // Build argument list from right to left:
        // (.GlobalEnv) -> nil
        let args2 = protect.protect((lib.rf_cons)(global_env, nil));
        // (path .GlobalEnv) -> nil
        let args1 = protect.protect((lib.rf_cons)(path_sexp, args2));

        // Build final call: sys.source(path, .GlobalEnv)
        let call = protect.protect((lib.rf_lcons)(sys_source_sym, args1));

        // Execute with R_ToplevelExec for safe error handling
        let mut payload = SourcePayload {
            call,
            env: *lib.r_baseenv,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(source_callback),
            &mut payload as *mut SourcePayload as *mut std::ffi::c_void,
        );

        if success == 0 {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "Error sourcing R profile (R error occurred)".to_string(),
            )));
        }

        Ok(())
    }
}

/// Payload for R_ToplevelExec callback.
struct SourcePayload {
    call: SEXP,
    env: SEXP,
    result: Option<SEXP>,
}

/// Callback for R_ToplevelExec - executes sys.source().
unsafe extern "C" fn source_callback(payload: *mut std::ffi::c_void) {
    let data = unsafe { &mut *(payload as *mut SourcePayload) };
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };

    // Evaluate sys.source() - if this throws, R_ToplevelExec catches it
    let result = unsafe { (lib.rf_eval)(data.call, data.env) };
    data.result = Some(result);
}

/// Return `true` if `val` is bound to a callable R function.
///
/// R functions can be any of three SEXPTYPEs: `CLOSXP` (user-defined closures),
/// `BUILTINSXP` (built-in functions with eager argument evaluation), and
/// `SPECIALSXP` (built-in functions with lazy argument evaluation — e.g. `if`,
/// `quote`). Mirrors the semantics of `Rf_isFunction`.
unsafe fn is_callable_function(val: SEXP, lib: &RLibrary) -> bool {
    if val == unsafe { *lib.r_unboundvalue } {
        return false;
    }
    let t = unsafe { (lib.rf_typeof)(val) };
    t == SexpType::ClosSxp as std::os::raw::c_int
        || t == SexpType::BuiltinSxp as std::os::raw::c_int
        || t == SexpType::SpecialSxp as std::os::raw::c_int
}

/// Install (intern) an R symbol by name.
unsafe fn install_symbol(name: &str) -> HarpResult<SEXP> {
    let lib = r_library()?;
    let name_cstring = CString::new(name).map_err(|_| HarpError::TypeMismatch {
        expected: "valid UTF-8".to_string(),
        actual: "string with null byte".to_string(),
    })?;
    unsafe { Ok((lib.rf_install)(name_cstring.as_ptr())) }
}

/// Call `.First()` if it is defined as a function in `.GlobalEnv`.
///
/// Mirrors R's `main.c` startup sequence: after profiles are sourced,
/// call `.First()` if it was defined by the user's `.Rprofile`.
/// Uses `R_ToplevelExec` for safe error handling.
pub fn call_dot_first() {
    match call_dot_first_impl() {
        Ok(called) => {
            if called {
                log::info!("Called .First()");
            }
        }
        Err(err) => {
            log::error!("Error calling .First(): {err}");
            eprintln!("Error in .First():\n{err}");
        }
    }
}

/// Call `.First.sys()` from the base namespace.
///
/// `.First.sys()` loads the default packages (utils, grDevices, etc.) via
/// `require()`. It must be called after `.First()` in the startup sequence.
/// Uses `R_ToplevelExec` for safe error handling.
pub fn call_dot_first_sys() {
    match call_dot_first_sys_impl() {
        Ok(called) => {
            if called {
                log::info!("Called .First.sys()");
            }
        }
        Err(err) => {
            log::error!("Error calling .First.sys(): {err}");
            eprintln!("Error in .First.sys():\n{err}");
        }
    }
}

fn call_dot_first_impl() -> HarpResult<bool> {
    let lib = r_library()?;

    unsafe {
        let mut protect = RProtect::new();
        let sym = install_symbol(".First")?;
        let global_env = *lib.r_globalenv;
        let val = (lib.rf_findvar)(sym, global_env);

        // Only call if .First is defined and bound to a function (any of the
        // three callable SEXPTYPEs — closure, builtin, or special).
        if !is_callable_function(val, lib) {
            return Ok(false);
        }

        // Build call: (.First) — lang1 equivalent
        let nil = r_nil_value()?;
        let call = protect.protect((lib.rf_lcons)(sym, nil));

        let mut payload = CallPayload {
            call,
            env: global_env,
        };
        let success = (lib.r_toplevelexec)(
            Some(call_callback),
            &mut payload as *mut CallPayload as *mut std::ffi::c_void,
        );

        if success == 0 {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "Error in .First() (R error occurred)".to_string(),
            )));
        }
    }
    Ok(true)
}

fn call_dot_first_sys_impl() -> HarpResult<bool> {
    let lib = r_library()?;

    unsafe {
        let mut protect = RProtect::new();
        let sym = install_symbol(".First.sys")?;
        // Look up in base namespace, eval in base env (mirrors R's main.c)
        let base_ns = *lib.r_basenamespace;
        let val = (lib.rf_findvar)(sym, base_ns);

        if !is_callable_function(val, lib) {
            return Ok(false);
        }

        let nil = r_nil_value()?;
        let call = protect.protect((lib.rf_lcons)(sym, nil));

        let mut payload = CallPayload { call, env: base_ns };
        let success = (lib.r_toplevelexec)(
            Some(call_callback),
            &mut payload as *mut CallPayload as *mut std::ffi::c_void,
        );

        if success == 0 {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "Error in .First.sys() (R error occurred)".to_string(),
            )));
        }
    }
    Ok(true)
}

/// Payload for call_callback.
struct CallPayload {
    call: SEXP,
    env: SEXP,
}

/// R_ToplevelExec callback for calling a no-arg R function.
unsafe extern "C" fn call_callback(payload: *mut std::ffi::c_void) {
    let data = unsafe { &mut *(payload as *mut CallPayload) };
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };
    unsafe { (lib.rf_eval)(data.call, data.env) };
}

/// Find site-level R profile.
fn find_site_r_profile(r_home: &Path) -> Option<PathBuf> {
    // 1. Try R_PROFILE environment variable
    if let Ok(path_str) = std::env::var("R_PROFILE") {
        let path = PathBuf::from(&path_str);
        if path.exists() {
            return Some(path);
        }
        log::warn!("`R_PROFILE` detected but '{path_str}' does not exist");
        return None;
    }

    // 2. Try arch-specific Rprofile.site (typically Windows: etc/x86/Rprofile.site)
    if let Ok(arch) = std::env::var("R_ARCH") {
        // Remove leading "/" if present
        let arch = arch.trim_start_matches('/');
        let path = r_home.join("etc").join(arch).join("Rprofile.site");
        if path.exists() {
            return Some(path);
        }
    }

    // 3. Try standard Rprofile.site location
    let path = r_home.join("etc").join("Rprofile.site");
    if path.exists() {
        return Some(path);
    }

    None
}

/// Find user-level R profile (.Rprofile).
fn find_user_r_profile() -> Option<PathBuf> {
    // 1. Try R_PROFILE_USER environment variable
    if let Ok(path_str) = std::env::var("R_PROFILE_USER") {
        let path = PathBuf::from(&path_str);
        if path.exists() {
            return Some(path);
        }
        log::warn!("`R_PROFILE_USER` detected but '{path_str}' does not exist");
        return None;
    }

    // 2. Try current directory .Rprofile
    if let Ok(cwd) = std::env::current_dir() {
        let path = cwd.join(".Rprofile");
        if path.exists() {
            return Some(path);
        }
    }

    // 3. Try user home directory .Rprofile
    if let Some(home) = r_user_home() {
        let path = home.join(".Rprofile");
        if path.exists() {
            return Some(path);
        }
    }

    None
}

/// Get the R user home directory.
///
/// - Windows: Uses `R_USER` environment variable
/// - Unix: Uses `HOME` environment variable
fn r_user_home() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var("R_USER").ok().map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_ignore_site_r_profile() {
        // Should ignore with --no-site-file
        assert!(should_ignore_site_r_profile(&[
            "--quiet".to_string(),
            "--no-site-file".to_string()
        ]));

        // Should ignore with --vanilla
        assert!(should_ignore_site_r_profile(&["--vanilla".to_string()]));

        // Should not ignore without the flags
        assert!(!should_ignore_site_r_profile(&[
            "--quiet".to_string(),
            "--no-save".to_string()
        ]));
    }

    #[test]
    fn test_should_ignore_user_r_profile() {
        // Should ignore with --no-init-file
        assert!(should_ignore_user_r_profile(&[
            "--quiet".to_string(),
            "--no-init-file".to_string()
        ]));

        // Should ignore with --vanilla
        assert!(should_ignore_user_r_profile(&["--vanilla".to_string()]));

        // Should not ignore without the flags
        assert!(!should_ignore_user_r_profile(&[
            "--quiet".to_string(),
            "--no-save".to_string()
        ]));
    }
}
