use crate::SEXP;
use crate::functions::r_library;
use std::sync::RwLock;

/// Tracks whether an independent condition-based hook reported an error.
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

/// The outcome R could reliably report for the previous command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CommandOutcome {
    Success,
    Failure,
    Unavailable,
}

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
/// This is called by condition-based error hooks when they detect an error.
#[allow(dead_code)]
pub fn mark_error_condition() {
    if let Ok(mut state) = CONDITION_ERROR_OCCURRED.write() {
        *state = true;
    }
}

/// Check the outcome of the current command.
///
/// Note: We rely on R's error handling mechanism rather than checking stderr output,
/// because many R functions (e.g., install.packages) write informational messages
/// to stderr that are not errors.
pub fn command_outcome() -> CommandOutcome {
    let had_condition = match CONDITION_ERROR_OCCURRED.read() {
        Ok(state) => *state,
        Err(_) => return CommandOutcome::Unavailable,
    };
    if had_condition {
        return CommandOutcome::Failure;
    }

    match check_r_error_state() {
        Some(true) => CommandOutcome::Failure,
        Some(false) => CommandOutcome::Success,
        None => CommandOutcome::Unavailable,
    }
}

/// Check whether the previous command failed, preserving the legacy fail-open behavior.
#[deprecated(note = "use command_outcome() instead")]
pub fn command_had_error() -> bool {
    outcome_had_error(command_outcome())
}

fn outcome_had_error(outcome: CommandOutcome) -> bool {
    outcome == CommandOutcome::Failure
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

/// Get the R code for installing the R `options(error)` wrapper.
///
/// Evaluate this after R startup and user profiles have finished, before the
/// first interactive command is returned to R. The wrapper chains any handler
/// already configured in `options(error)`.
///
/// Call this from the application layer (e.g., arf-console) and use arf-harp's
/// eval_string to evaluate the returned code.
pub fn global_error_handler_code() -> &'static str {
    GLOBAL_ERROR_HANDLER_CODE
}

/// Mark the global error handler as initialized.
///
/// Call this after successfully evaluating `global_error_handler_code()`.
/// This enables R-side error state checking in `command_outcome()`.
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

/// R code to install the `options(error)` wrapper.
///
/// This uses `options(error = ...)` to intercept all errors after they occur.
/// The error handler is called at the end of R's error handling, right before
/// returning to the prompt. This catches all errors, including rlang/dplyr errors.
///
/// The handler stores the error state in a dedicated environment that Rust can
/// inspect through R's C API.
const GLOBAL_ERROR_HANDLER_CODE: &str = r#"
base::local({
    state <- base::new.env(parent = base::emptyenv())
    base::assign("had_error", FALSE, envir = state)
    previous <- base::getOption("error")

    # Keep both values in the closure so removal of the observable global state
    # does not interfere with normal R error handling or a user's handler.
    handler <- base::local({
        captured_state <- state
        captured_previous <- previous
        function() {
            if (base::exists("had_error", envir = captured_state, inherits = FALSE) &&
                !base::bindingIsActive("had_error", captured_state) &&
                !base::bindingIsLocked("had_error", captured_state)) {
                base::assign("had_error", TRUE, envir = captured_state)
            }
            if (!base::is.null(captured_previous)) {
                if (base::is.function(captured_previous)) {
                    captured_previous()
                } else {
                    base::eval(captured_previous, envir = base::globalenv())
                }
            }
            base::invisible(NULL)
        }
    })
    base::assign("handler", handler, envir = state)
    base::assign(".arf_error_state", state, envir = base::globalenv())
    base::options(error = handler)
    base::invisible(NULL)
})
"#;

/// Check R's tracked error state and verify that the wrapper still owns options(error).
///
/// This reads `.arf_error_state$had_error` from the state environment exposed in
/// the global environment.
/// The `options(error)` wrapper sets this to TRUE when an error occurs.
///
/// # Safety
/// R must be initialized and the global error handler must be set up
/// before this function returns meaningful results.
fn check_r_error_state() -> Option<bool> {
    // Don't check R state if the handler hasn't been initialized yet
    if !is_global_error_handler_initialized() {
        return None;
    }

    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return None,
    };

    unsafe {
        // Look up the binding directly in global environment.
        let arf_error_state_sym = {
            let name = std::ffi::CString::new(".arf_error_state").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        let global_env = *lib.r_globalenv;
        if !binding_exists(lib, global_env, arf_error_state_sym)
            || (lib.r_binding_is_active)(arf_error_state_sym, global_env) != 0
        {
            return None;
        }
        let state_env = (lib.rf_findvar_in_frame)(global_env, arf_error_state_sym);

        // Check if the environment exists
        if state_env.is_null()
            || state_env == *lib.r_unboundvalue
            || (lib.rf_typeof)(state_env) != crate::SexpType::EnvSxp as i32
        {
            return None;
        }

        // Look up had_error in the state environment
        let had_error_sym = {
            let name = std::ffi::CString::new("had_error").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        if !binding_exists(lib, state_env, had_error_sym)
            || (lib.r_binding_is_active)(had_error_sym, state_env) != 0
            || (lib.r_binding_is_locked)(had_error_sym, state_env) != 0
        {
            return None;
        }
        let had_error = (lib.rf_findvar_in_frame)(state_env, had_error_sym);

        let had_error_value = if had_error.is_null() || had_error == *lib.r_unboundvalue {
            None
        } else {
            let logical_ptr = if (lib.rf_typeof)(had_error) == crate::SexpType::LglSxp as i32
                && (lib.rf_xlength)(had_error) == 1
            {
                (lib.logical)(had_error)
            } else {
                std::ptr::null_mut()
            };
            (!logical_ptr.is_null()).then(|| *logical_ptr)
        };

        match valid_had_error_value(had_error_value) {
            Some(true) => return Some(true),
            Some(false) => {}
            None => return None,
        }

        // R represents a function-valued options(error) as a call whose head
        // is the handler closure. Compare that closure's SEXP identity.
        let handler_sym = {
            let name = std::ffi::CString::new("handler").unwrap();
            (lib.rf_install)(name.as_ptr())
        };
        if !binding_exists(lib, state_env, handler_sym)
            || (lib.r_binding_is_active)(handler_sym, state_env) != 0
        {
            return None;
        }
        let handler = (lib.rf_findvar_in_frame)(state_env, handler_sym);
        if handler.is_null()
            || handler == *lib.r_unboundvalue
            || handler == *lib.r_nilvalue
            || (lib.rf_typeof)(handler) != crate::SexpType::ClosSxp as i32
        {
            return None;
        }

        let error_sym = {
            let name = std::ffi::CString::new("error").unwrap();
            (lib.rf_install)(name.as_ptr())
        };
        let current_option = (lib.rf_get_option1)(error_sym);
        if current_option.is_null()
            || current_option == *lib.r_nilvalue
            || (lib.rf_typeof)(current_option) != crate::SexpType::LangSxp as i32
            || (lib.car)(current_option) != handler
        {
            return None;
        }

        Some(false)
    }
}

/// Check whether a binding exists in this frame without resolving its value.
/// This avoids evaluating active bindings before inspecting their properties.
unsafe fn binding_exists(lib: &crate::RLibrary, env: SEXP, symbol: SEXP) -> bool {
    unsafe { (lib.r_exists_var_in_frame)(env, symbol) != 0 }
}

fn valid_had_error_value(value: Option<i32>) -> Option<bool> {
    match value {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => None,
    }
}

/// Reset the R error state.
///
/// This should be called before each command to reset the error tracking.
/// Sets `.arf_error_state$had_error` to FALSE when its binding is safe to write.
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
        // Look up the binding directly in global environment.
        let arf_error_state_sym = {
            let name = std::ffi::CString::new(".arf_error_state").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        let global_env = *lib.r_globalenv;
        if !binding_exists(lib, global_env, arf_error_state_sym)
            || (lib.r_binding_is_active)(arf_error_state_sym, global_env) != 0
        {
            return;
        }
        let state_env = (lib.rf_findvar_in_frame)(global_env, arf_error_state_sym);

        // If the environment doesn't exist, nothing to reset
        if state_env.is_null()
            || state_env == *lib.r_unboundvalue
            || (lib.rf_typeof)(state_env) != crate::SexpType::EnvSxp as i32
        {
            log::trace!("reset_r_error_state: .arf_error_state not found");
            return;
        }

        // Set had_error to FALSE using Rf_defineVar
        let had_error_sym = {
            let name = std::ffi::CString::new("had_error").unwrap();
            (lib.rf_install)(name.as_ptr())
        };

        if !binding_exists(lib, state_env, had_error_sym)
            || (lib.r_binding_is_active)(had_error_sym, state_env) != 0
            || (lib.r_binding_is_locked)(had_error_sym, state_env) != 0
        {
            log::trace!("reset_r_error_state: had_error binding is unavailable");
            return;
        }

        // Create FALSE value (0)
        let false_val = (lib.rf_protect)((lib.rf_scalarlogical)(0));

        // Set had_error = FALSE in the state environment
        (lib.rf_definevar)(had_error_sym, false_val, state_env);
        (lib.rf_unprotect)(1);
        log::trace!("reset_r_error_state: set had_error = FALSE");
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandOutcome, outcome_had_error, valid_had_error_value};

    #[test]
    fn deprecated_boolean_projection_only_reports_known_failure() {
        assert!(outcome_had_error(CommandOutcome::Failure));
        assert!(!outcome_had_error(CommandOutcome::Success));
        assert!(!outcome_had_error(CommandOutcome::Unavailable));
    }

    #[test]
    fn malformed_error_values_are_unavailable() {
        assert_eq!(valid_had_error_value(Some(0)), Some(false));
        assert_eq!(valid_had_error_value(Some(1)), Some(true));
        assert_eq!(valid_had_error_value(None), None);
        assert_eq!(valid_had_error_value(Some(-1)), None);
        assert_eq!(valid_had_error_value(Some(2)), None);
    }
}
