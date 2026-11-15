//! Observe escaping conditions without changing R's error or interrupt handling.

use std::cell::Cell;

use arf_libr::{SEXP, SexpType, r_library};

use crate::ffi::top_level_exec;

thread_local! {
    // R has one lifetime in this process. Preserve the private environment for
    // that lifetime rather than holding a PROTECT entry across REPL callbacks.
    static STATE: Cell<SEXP> = const { Cell::new(std::ptr::null_mut()) };
    static ATTEMPTED: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnhandledCondition {
    Error,
    Interrupt,
}

/// Reinstall the prepared handler list on the native REPL's active stack.
pub const ACTIVATE: &str = r#"base::local({ handlers <- base::globalCallingHandlers(NULL); base::do.call(base::globalCallingHandlers, handlers); base::invisible(NULL) })
"#;

/// Prepare once at a top-level prompt, preserving existing handlers ahead of ours.
/// Return whether the caller must feed ACTIVATE through the native REPL.
/// R_ToplevelExec restores the old active handler stack on return, so the
/// registration must be committed by R itself with no Rust frames beneath it.
pub fn initialize() -> Result<bool, String> {
    if ATTEMPTED.replace(true) {
        return Ok(false);
    }
    let state = crate::eval_string_in_base(
        r#"
base::local({
    if (!base::exists("globalCallingHandlers", envir = base::baseenv())) {
        return(base::invisible(NULL))
    }
    state <- base::new.env(parent = base::emptyenv())
    state$outcome <- 0L
    # Calling handlers also see signalCondition(), which can return normally.
    # R invokes task callbacks after evaluation, printing, and warnings succeed;
    # keep a condition only when its expression did not complete normally.
    base::addTaskCallback(function(expr, value, ok, visible) {
        if (ok) state$outcome <- 0L
        TRUE
    }, name = "arf-condition-observer")
    on_error <- function(e) { state$outcome <- 1L; base::invisible(NULL) }
    on_interrupt <- function(e) { state$outcome <- 2L; base::invisible(NULL) }
    handlers <- base::globalCallingHandlers(NULL)
    handlers <- base::c(handlers, base::list(error = on_error, interrupt = on_interrupt))
    base::do.call(base::globalCallingHandlers, handlers)
    base::invisible(state)
})
"#,
    )
    .map_err(|error| error.to_string())?;
    let lib = r_library().map_err(|error| error.to_string())?;
    let value = state.sexp();
    if unsafe { (lib.rf_typeof)(value) } != SexpType::EnvSxp as i32 {
        return Ok(false);
    }
    // SAFETY: the RObject protects value; the callback captures only Copy data.
    unsafe { top_level_exec(move || (lib.r_preserve_object)(value)) }
        .map_err(|error| error.to_string())?;
    STATE.set(value);
    Ok(true)
}

/// Begin a visible user-code execution. Removed global handlers stay removed.
pub fn reset() {
    let state = STATE.get();
    let Ok(lib) = r_library() else { return };
    if state.is_null() {
        return;
    }
    // SAFETY: state is preserved, and no Rust-owned values enter the callback.
    let _ = unsafe {
        top_level_exec(move || {
            let symbol = (lib.rf_install)(c"outcome".as_ptr());
            let zero = (lib.rf_protect)((lib.rf_scalarinteger)(0));
            (lib.rf_definevar)(symbol, zero, state);
            (lib.rf_unprotect)(1);
        })
    };
}

/// Read the last condition whose expression did not complete normally.
pub fn outcome() -> Option<UnhandledCondition> {
    let state = STATE.get();
    let lib = r_library().ok()?;
    if state.is_null() {
        return None;
    }
    // SAFETY: the private environment is preserved; all R calls are protected.
    let value = unsafe {
        top_level_exec(move || {
            let symbol = (lib.rf_install)(c"outcome".as_ptr());
            if (lib.r_binding_is_active)(symbol, state) != 0 {
                return 0;
            }
            let value = (lib.rf_findvar_in_frame)(state, symbol);
            if (lib.rf_typeof)(value) != SexpType::IntSxp as i32 || (lib.rf_length)(value) != 1 {
                return 0;
            }
            *(lib.integer)(value)
        })
    }
    .ok()?;
    match value {
        1 => Some(UnhandledCondition::Error),
        2 => Some(UnhandledCondition::Interrupt),
        _ => None,
    }
}
