//! Integration tests for R startup hook functions.

use arf_harp::{call_dot_first, call_dot_first_sys, eval_string};
use once_cell::sync::OnceCell;
use std::sync::Mutex;

static R_LOCK: OnceCell<Mutex<()>> = OnceCell::new();

fn ensure_r_initialized() -> bool {
    static R_INITIALIZED: OnceCell<bool> = OnceCell::new();

    *R_INITIALIZED.get_or_init(|| unsafe {
        match arf_libr::initialize_r() {
            Ok(()) => true,
            Err(e) => {
                eprintln!("Failed to initialize R: {}", e);
                false
            }
        }
    })
}

fn with_r<F, T>(f: F) -> Option<T>
where
    F: FnOnce() -> T,
{
    if !ensure_r_initialized() {
        return None;
    }
    let lock = R_LOCK.get_or_init(|| Mutex::new(()));
    // Use into_inner() to recover from a poisoned lock caused by a previous panic.
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    Some(f())
}

/// Check that LD_LIBRARY_PATH includes the R library directory.
/// Tests that require package loading (e.g. utils, methods) must be skipped
/// when this is not set, because `initialize_r()` cannot re-exec the process
/// as the binary does via `ensure_ld_library_path()`.
fn ld_library_path_is_set() -> bool {
    let Ok(lib_path) = arf_libr::find_r_library() else {
        return false;
    };
    let Some(lib_dir) = lib_path.parent() else {
        return false;
    };
    let lib_dir_str = lib_dir.to_string_lossy();
    let current = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
    current.split(':').any(|p| p == lib_dir_str.as_ref())
}

#[test]
fn test_call_dot_first_noop_when_undefined() {
    // .First is not defined after plain R initialization — call should be a no-op.
    with_r(|| {
        eval_string("rm(list = intersect('.First', ls(.GlobalEnv)))").ok();
        // Must not panic or error
        call_dot_first();
    });
}

#[test]
fn test_call_dot_first_invokes_function() {
    with_r(|| {
        // Define .First in GlobalEnv with a detectable side effect
        eval_string(".arf_test_first_called <- FALSE").unwrap();
        eval_string(".First <- function() { .arf_test_first_called <<- TRUE }").unwrap();

        call_dot_first();

        eval_string("stopifnot(isTRUE(.arf_test_first_called))")
            .expect(".First() should have been called and set .arf_test_first_called");

        // Clean up
        eval_string("rm('.First', '.arf_test_first_called', envir = .GlobalEnv)").ok();
    });
}

#[test]
fn test_call_dot_first_skips_non_function() {
    with_r(|| {
        // .First is defined but is not a function — should be skipped silently
        eval_string(".First <- 42L").unwrap();

        call_dot_first(); // must not panic or error

        eval_string("rm('.First', envir = .GlobalEnv)").ok();
    });
}

#[test]
fn test_call_dot_first_sys_does_not_error() {
    // .First.sys() loads default packages via require(). On Linux, R's
    // setup_Rmainloop() already called it during initialization, so calling
    // it again exercises the idempotent require() path.
    with_r(|| {
        call_dot_first_sys(); // must not panic or error
    });
}

#[test]
fn test_call_dot_first_sys_loads_default_packages() {
    // After call_dot_first_sys(), the standard default packages should be attached.
    // Requires LD_LIBRARY_PATH to be set so package shared libraries can be found.
    if !ld_library_path_is_set() {
        eprintln!(
            "Skipping test_call_dot_first_sys_loads_default_packages: \
             LD_LIBRARY_PATH not set. Run with LD_LIBRARY_PATH pointing to R's lib dir."
        );
        return;
    }

    with_r(|| {
        call_dot_first_sys();
        eval_string("stopifnot(isNamespaceLoaded('utils'))")
            .expect("utils namespace should be loaded after .First.sys()");
    });
}
