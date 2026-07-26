use crate::functions::r_library;
use std::sync::RwLock;

/// Tracks whether an error condition was signaled via globalCallingHandlers.
/// This catches rlang/dplyr errors that output to stdout instead of stderr.
static CONDITION_ERROR_OCCURRED: RwLock<bool> = RwLock::new(false);

/// Tracks whether stderr output should be suppressed.
/// When true, r_write_console_ex silently drops stderr output (otype != 0).
/// Used during completion to prevent error messages from interfering with the UI.
/// This matches radian's suppress_stderr pattern.
static SUPPRESS_STDERR: RwLock<bool> = RwLock::new(false);

/// Tracks whether the global error handler has been initialized.
/// This prevents calling R functions before the handler environment exists.
static GLOBAL_ERROR_HANDLER_INITIALIZED: RwLock<bool> = RwLock::new(false);

/// Reset the error state for the current command.
///
/// Call this before executing a new command to track errors accurately.
pub fn reset_command_error_state() {
    if let Ok(mut state) = CONDITION_ERROR_OCCURRED.write() {
        *state = false;
    }
    // Also reset the R-side error state
    reset_r_error_state();
}

/// Mark that an error condition was signaled.
///
/// This is called from the global error handler set up by `initialize_global_error_handler()`.
#[allow(dead_code)]
pub fn mark_error_condition() {
    if let Ok(mut state) = CONDITION_ERROR_OCCURRED.write() {
        *state = true;
    }
}

/// Check if the current command produced an error.
///
/// Returns `true` if either:
/// - An error condition was signaled via R's condition system (globalCallingHandlers), OR
/// - The R-side error state was set via options(error = ...) handler
///
/// Note: We rely on R's error handling mechanism rather than checking stderr output,
/// because many R functions (e.g., install.packages) write informational messages
/// to stderr that are not errors.
pub fn command_had_error() -> bool {
    let had_condition = CONDITION_ERROR_OCCURRED.read().map(|s| *s).unwrap_or(false);
    let had_r_error = check_r_error_state();
    had_condition || had_r_error
}

/// Suppress stderr output from R.
///
/// While suppressed, `r_write_console_ex` will silently drop stderr output
/// (otype != 0). Stdout output is not affected.
///
/// This is used during completion to prevent error messages from interfering
/// with the terminal display, matching radian's suppress_stderr pattern.
///
/// Use `restore_stderr()` to re-enable stderr output.
pub fn suppress_stderr() {
    if let Ok(mut state) = SUPPRESS_STDERR.write() {
        *state = true;
    }
}

/// Restore stderr output after suppression.
///
/// Call this after `suppress_stderr()` to re-enable normal stderr output.
pub fn restore_stderr() {
    if let Ok(mut state) = SUPPRESS_STDERR.write() {
        *state = false;
    }
}

/// Check if stderr output is currently suppressed.
pub(super) fn is_stderr_suppressed() -> bool {
    SUPPRESS_STDERR.read().map(|s| *s).unwrap_or(false)
}

/// Get the R code for setting up the global error handler.
///
/// This should be evaluated after R is initialized but before the main loop starts.
/// It sets up `globalCallingHandlers()` (R >= 4.0) to track error conditions
/// that may output to stdout instead of stderr.
///
/// Call this from the application layer (e.g., arf-console) and use arf-harp's
/// eval_string to evaluate the returned code.
pub fn global_error_handler_code() -> &'static str {
    GLOBAL_ERROR_HANDLER_CODE
}

/// Mark the global error handler as initialized.
///
/// Call this after successfully evaluating `global_error_handler_code()`.
/// This enables R-side error state checking in `command_had_error()`.
pub fn mark_global_error_handler_initialized() {
    if let Ok(mut state) = GLOBAL_ERROR_HANDLER_INITIALIZED.write() {
        *state = true;
    }
}

/// Check if the global error handler has been initialized.
fn is_global_error_handler_initialized() -> bool {
    GLOBAL_ERROR_HANDLER_INITIALIZED
        .read()
        .map(|s| *s)
        .unwrap_or(false)
}

/// R code to set up the global error handler.
///
/// This uses `options(error = ...)` to intercept all errors after they occur.
/// The error handler is called at the end of R's error handling, right before
/// returning to the prompt. This catches all errors, including rlang/dplyr errors.
///
/// Note: globalCallingHandlers() doesn't work reliably in embedded R because
/// errors caught by R_ToplevelExec or similar mechanisms bypass the condition system.
///
/// The handler stores the error state in an environment variable that we can
/// check from Rust using Rf_findVar.
const GLOBAL_ERROR_HANDLER_CODE: &str = r#"
local({
    # Create an environment to store error state
    .arf_error_state <- new.env(parent = emptyenv())
    .arf_error_state$had_error <- FALSE

    # Store it in global environment for persistence
    assign(".arf_error_state", .arf_error_state, envir = globalenv())

    # Store the user's previous error handler (if any) so we can chain to it
    prev_handler <- getOption("error")
    assign(".arf_prev_error_handler", prev_handler, envir = globalenv())

    # Set up our error handler using options(error = ...)
    # This is called at the END of R's error handling, just before returning to prompt
    options(error = function() {
        # Mark that an error occurred
        env <- get(".arf_error_state", envir = globalenv())
        env$had_error <- TRUE

        # Chain to the previous handler if it exists
        prev <- get(".arf_prev_error_handler", envir = globalenv())
        if (!is.null(prev)) {
            if (is.function(prev)) prev() else eval(prev, envir = globalenv())
        }
    })

    invisible(NULL)
})
"#;

/// Check if the R error state indicates an error occurred.
///
/// This reads `.arf_error_state$had_error` from the global environment.
/// The globalCallingHandlers error handler sets this to TRUE when an error occurs.
///
/// # Safety
/// R must be initialized and the global error handler must be set up
/// before this function returns meaningful results.
fn check_r_error_state() -> bool {
    // Don't check R state if the handler hasn't been initialized yet
    if !is_global_error_handler_initialized() {
        return false;
    }

    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return false,
    };

    unsafe {
        // Look up .arf_error_state in global environment using Rf_findVar
        let arf_error_state_sym = {
            let name = std::ffi::CString::new(".arf_error_state").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        let global_env = *lib.r_globalenv;
        let state_env = (lib.rf_findvar)(arf_error_state_sym, global_env);

        // Check if the environment exists
        if state_env.is_null() || state_env == *lib.r_unboundvalue {
            return false;
        }

        // Look up had_error in the state environment
        let had_error_sym = {
            let name = std::ffi::CString::new("had_error").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        let had_error = (lib.rf_findvar)(had_error_sym, state_env);

        if had_error.is_null() || had_error == *lib.r_unboundvalue {
            return false;
        }

        // Check if it's TRUE (logical vector with value != 0)
        let logical_ptr = (lib.logical)(had_error);
        if !logical_ptr.is_null() {
            return *logical_ptr != 0;
        }

        false
    }
}

/// Reset the R error state.
///
/// This should be called before each command to reset the error tracking.
/// Sets `.arf_error_state$had_error` to FALSE.
///
/// # Safety
/// R must be initialized and the global error handler must be set up
/// before this function has any effect.
fn reset_r_error_state() {
    // Don't try to reset R state if the handler hasn't been initialized yet
    if !is_global_error_handler_initialized() {
        return;
    }

    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };

    unsafe {
        // Look up .arf_error_state in global environment
        let arf_error_state_sym = {
            let name = std::ffi::CString::new(".arf_error_state").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        let global_env = *lib.r_globalenv;
        let state_env = (lib.rf_findvar)(arf_error_state_sym, global_env);

        // If the environment doesn't exist, nothing to reset
        if state_env.is_null() || state_env == *lib.r_unboundvalue {
            log::trace!("reset_r_error_state: .arf_error_state not found");
            return;
        }

        // Set had_error to FALSE using Rf_defineVar
        let had_error_sym = {
            let name = std::ffi::CString::new("had_error").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        // Create FALSE value (0)
        let false_val = (lib.rf_scalarlogical)(0);

        // Set had_error = FALSE in the state environment
        (lib.rf_definevar)(had_error_sym, false_val, state_env);
        log::trace!("reset_r_error_state: set had_error = FALSE");
    }
}
