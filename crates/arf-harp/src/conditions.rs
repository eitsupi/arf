//! Observe escaping conditions without changing R's error or interrupt handling.

use std::cell::Cell;

use arf_libr::{SEXP, SexpType, r_library};

use crate::ffi::top_level_exec;

thread_local! {
    // R has one lifetime in this process. Preserve the private environment for
    // that lifetime rather than holding a PROTECT entry across REPL callbacks.
    static STATE: Cell<SEXP> = const { Cell::new(std::ptr::null_mut()) };
    static READY: Cell<bool> = const { Cell::new(false) };
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static DESIRED: Cell<bool> = const { Cell::new(false) };
    static PENDING: Cell<SEXP> = const { Cell::new(std::ptr::null_mut()) };
    static ATTEMPTED: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnhandledCondition {
    Error,
    Interrupt,
}

/// Reconcile at a real top-level prompt, then run the returned action only
/// after every owned ReadConsole frame and input guard has been dropped.
/// Bare eval (as in ark's module initialization) updates the native handler
/// stack without submitting a REPL expression or running user task callbacks.
pub fn update(enabled: bool) -> Result<Option<unsafe extern "C-unwind" fn()>, String> {
    if enabled == ACTIVE.get() {
        return Ok(None);
    }
    // A nested reader can reuse the command prompt text. Never mutate global
    // handlers while a user evaluation (and its local handlers) is on the stack.
    if arf_libr::frame_depth_protected()? != 0 {
        return Ok(None);
    }
    if STATE.get().is_null() && !prepare()? {
        return Ok(None);
    }
    let state = STATE.get();
    let lib = r_library().map_err(|error| error.to_string())?;
    let name = if enabled { c"install" } else { c"uninstall" };
    let name_ptr = name.as_ptr();
    // SAFETY: the preserved environment owns both prebuilt calls. Only Copy
    // values cross the protected callback; no R evaluation takes place here.
    let call = unsafe {
        top_level_exec(move || {
            let symbol = (lib.rf_install)(name_ptr);
            (lib.rf_findvar_in_frame)(state, symbol)
        })
    }
    .map_err(|error| error.to_string())?;
    PENDING.set(call);
    DESIRED.set(enabled);
    Ok(Some(run_pending))
}

unsafe extern "C-unwind" fn run_pending() {
    let call = PENDING.replace(std::ptr::null_mut());
    let Ok(lib) = r_library() else { return };
    if call.is_null() {
        return;
    }
    // Mark before eval so an error/interrupt does not cause an endless retry
    // at the next prompt. A later IPC stop/start can attempt registration again.
    ACTIVE.set(DESIRED.get());
    READY.set(false);
    // SAFETY: the caller is the destructor-free outer ReadConsole frame.
    // The call and its closure are reachable from the preserved environment.
    // No owned Rust values or guards may live across this bare evaluation;
    // errors and interrupts jump to R's existing top-level context.
    unsafe {
        (lib.rf_eval)(call, *lib.r_baseenv);
    }
    READY.set(DESIRED.get());
}

fn prepare() -> Result<bool, String> {
    if ATTEMPTED.replace(true) {
        return Ok(!STATE.get().is_null());
    }
    let state = crate::eval_string_in_base(
        r#"
base::local({
    if (!base::exists("globalCallingHandlers", envir = base::baseenv())) {
        return(base::invisible(NULL))
    }
    state <- base::new.env(parent = base::emptyenv())
    state$outcome <- 0L
    state$callback_name <- NULL
    on_error <- function(e) { state$outcome <- 1L; base::invisible(NULL) }
    on_interrupt <- function(e) { state$outcome <- 2L; base::invisible(NULL) }
    install <- function() {
        handlers <- base::globalCallingHandlers(NULL)
        handlers <- base::c(handlers, base::list(error = on_error, interrupt = on_interrupt))
        base::do.call(base::globalCallingHandlers, handlers)
        # Conditions may return normally; only successful native REPL tasks
        # clear them. Protected internal evaluations never run this callback.
        names <- base::make.unique(base::c(base::getTaskCallbackNames(), "arf-condition-observer"))
        state$callback_name <- names[[base::length(names)]]
        base::addTaskCallback(function(expr, value, ok, visible) {
            if (ok) state$outcome <- 0L
            TRUE
        }, name = state$callback_name)
        state$outcome <- 0L
        base::invisible(NULL)
    }
    uninstall <- function() {
        handlers <- base::globalCallingHandlers(NULL)
        handlers <- base::Filter(function(h) {
            !base::identical(h, on_error) && !base::identical(h, on_interrupt)
        }, handlers)
        base::do.call(base::globalCallingHandlers, handlers)
        if (!base::is.null(state$callback_name) &&
            state$callback_name %in% base::getTaskCallbackNames()) {
            base::removeTaskCallback(state$callback_name)
        }
        state$callback_name <- NULL
        state$outcome <- 0L
        base::invisible(NULL)
    }
    state$install <- base::as.call(base::list(install))
    state$uninstall <- base::as.call(base::list(uninstall))
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
    if !READY.get() {
        return;
    }
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
    if !READY.get() {
        return None;
    }
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
