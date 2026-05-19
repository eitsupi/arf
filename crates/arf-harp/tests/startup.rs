//! Integration tests for R startup hook functions.

mod common;

use arf_harp::{call_dot_first, call_dot_first_sys, eval_string};
use common::with_r;
#[cfg(not(windows))]
use common::ld_library_path_is_set;

#[test]
fn test_call_dot_first_noop_when_undefined() {
    // .First is not defined after plain R initialization — call must return
    // false (skipped) and must not panic or error.
    with_r(|| {
        eval_string("try(rm('.First', envir = .GlobalEnv), silent = TRUE)").ok();
        assert!(
            !call_dot_first(),
            ".First is undefined, so call_dot_first() must report skipped"
        );
    });
}

#[test]
fn test_call_dot_first_invokes_function() {
    with_r(|| {
        // Define .First in GlobalEnv with a detectable side effect
        eval_string(".arf_test_first_called <- FALSE").unwrap();
        eval_string(".First <- function() { .arf_test_first_called <<- TRUE }").unwrap();

        assert!(
            call_dot_first(),
            "closure .First must be reported as invoked"
        );

        eval_string("stopifnot(isTRUE(.arf_test_first_called))")
            .expect(".First() should have been called and set .arf_test_first_called");

        // Clean up
        eval_string("rm('.First', '.arf_test_first_called', envir = .GlobalEnv)").ok();
    });
}

#[test]
fn test_call_dot_first_skips_non_function() {
    with_r(|| {
        // .First is defined but is not a function — must be skipped silently.
        eval_string(".First <- 42L").unwrap();

        assert!(
            !call_dot_first(),
            "non-function .First must be reported as skipped"
        );

        eval_string("rm('.First', envir = .GlobalEnv)").ok();
    });
}

#[test]
fn test_call_dot_first_accepts_builtin() {
    // .First can legitimately be bound to a BUILTINSXP primitive (e.g. `sum`).
    // The callable-function detection must accept builtins, not only closures.
    // `sum()` with no args returns 0L, which is a safe no-side-effect call.
    with_r(|| {
        eval_string(".First <- sum").unwrap();
        eval_string("stopifnot(typeof(.First) == 'builtin')")
            .expect(".First should be bound to a BUILTINSXP primitive");

        assert!(
            call_dot_first(),
            "builtin .First must be reported as invoked — \
             returning false would indicate it was silently skipped"
        );

        eval_string("rm('.First', envir = .GlobalEnv)").ok();
    });
}

#[test]
fn test_call_dot_first_sys_does_not_error() {
    // .First.sys() loads default packages via require(). On Linux, R's
    // setup_Rmainloop() already called it during initialization, so calling
    // it again exercises the idempotent require() path. It must be reported
    // as invoked since the base namespace always defines it.
    with_r(|| {
        assert!(
            call_dot_first_sys(),
            ".First.sys from R's base namespace must always be reported as invoked"
        );
    });
}

#[test]
fn test_call_dot_first_sys_evaluates_in_base_env() {
    // Guard against regressing the eval environment used for .First.sys().
    // R's normal startup evaluates the call in R_BaseEnv so parent.frame()
    // inside .First.sys() returns R_BaseEnv. Evaluating in R_BaseNamespace
    // instead would silently change that contract.
    //
    // The test overrides base::.First.sys with a probe that records its
    // caller frame, invokes call_dot_first_sys(), and asserts the recorded
    // frame is identical to baseenv(). Every operation uses base-package
    // primitives so no extra packages (utils / methods) are required.
    with_r(|| {
        eval_string(
            "local({ \
                 ns <- asNamespace('base'); \
                 unlockBinding('.First.sys', ns); \
                 .arf_saved_first_sys <<- ns$.First.sys; \
                 ns$.First.sys <- function() { \
                     assign('.arf_captured_frame', parent.frame(), envir = globalenv()) \
                 } \
             })",
        )
        .expect("should install .First.sys probe");

        let invoked = call_dot_first_sys();

        // Always restore the original binding, even if the assertions below fail.
        let restored = eval_string(
            "local({ \
                 ns <- asNamespace('base'); \
                 ns$.First.sys <- .arf_saved_first_sys; \
                 lockBinding('.First.sys', ns); \
                 rm('.arf_saved_first_sys', envir = globalenv()) \
             })",
        );

        assert!(invoked, ".First.sys must be reported as invoked");
        eval_string("stopifnot(identical(.arf_captured_frame, baseenv()))")
            .expect(".First.sys must be evaluated in R_BaseEnv");
        eval_string("rm('.arf_captured_frame', envir = globalenv())").ok();
        restored.expect(".First.sys restoration should succeed");
    });
}

// Windows does not use LD_LIBRARY_PATH, so package shared libraries are located
// differently. Exclude this test on Windows until a Windows-equivalent check exists.
#[cfg(not(windows))]
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
        assert!(
            call_dot_first_sys(),
            ".First.sys from R's base namespace must always be reported as invoked"
        );
        eval_string("stopifnot(isNamespaceLoaded('utils'))")
            .expect("utils namespace should be loaded after .First.sys()");
    });
}
