//! Low-level R FFI helpers shared by the evaluation paths in this crate.

use crate::error::{HarpError, HarpResult};
use arf_libr::{SEXP, r_library};
use std::ffi::CString;

#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(test)]
static R_EVALUATION_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Number of expressions evaluated through [`eval_callback`] so far.
///
/// Tests compare readings taken before and after an operation to verify
/// how many R evaluations it performed.
#[cfg(test)]
pub(crate) fn r_evaluation_count() -> usize {
    R_EVALUATION_COUNT.load(Ordering::Relaxed)
}

/// Install (intern) an R symbol by name.
pub(crate) unsafe fn install_symbol(name: &str) -> HarpResult<SEXP> {
    let lib = r_library()?;
    let name_cstring = CString::new(name).map_err(|_| HarpError::TypeMismatch {
        expected: "valid UTF-8".to_string(),
        actual: "string with null byte".to_string(),
    })?;
    // SAFETY: rf_install is safe to call with a valid C string
    unsafe { Ok((lib.rf_install)(name_cstring.as_ptr())) }
}

/// Payload for the `R_ToplevelExec` evaluation callback.
pub(crate) struct EvalPayload {
    pub(crate) expr: SEXP,
    pub(crate) env: SEXP,
    pub(crate) result: Option<SEXP>,
}

/// Callback for `R_ToplevelExec` - evaluates the expression.
///
/// If the evaluation raises an R error, `R_ToplevelExec` catches it and
/// `result` stays `None`.
pub(crate) unsafe extern "C-unwind" fn eval_callback(payload: *mut std::ffi::c_void) {
    let data = unsafe { &mut *(payload as *mut EvalPayload) };
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };
    #[cfg(test)]
    R_EVALUATION_COUNT.fetch_add(1, Ordering::Relaxed);
    let result = unsafe { (lib.rf_eval)(data.expr, data.env) };
    data.result = Some(result);
}

struct TopLevelPayload<F, T> {
    closure: Option<F>,
    result: Option<T>,
}

unsafe extern "C-unwind" fn top_level_trampoline<F, T>(data: *mut std::ffi::c_void)
where
    F: FnOnce() -> T,
{
    // SAFETY: `data` points to the payload owned by `top_level_exec`, which
    // outlives the `R_ToplevelExec` call.
    let payload = unsafe { &mut *(data as *mut TopLevelPayload<F, T>) };
    if let Some(closure) = payload.closure.take() {
        payload.result = Some(closure());
    }
}

/// Run `closure` under `R_ToplevelExec`, turning an R error into an `Err`.
///
/// Calls into R that can raise an error (allocation, encoding translation,
/// ALTREP element access, ...) `longjmp` over every frame between the call and
/// the nearest R context. That is harmless only for frames without
/// destructors, so the contract is the same as for `top_level_exec` in ark:
/// **the closure must not hold a value with a destructor across a call into
/// R.** Anything owned (a `String`, a `Vec`, a guard) stays outside, and may
/// be written to through a raw pointer only after the last call into R that
/// can fail. The `Copy` bounds make the compiler enforce this for what the
/// closure captures and returns; locals inside the closure are up to the
/// caller.
///
/// Limitations:
/// - If the closure raises an R error, R still prints its default error
///   message to the console, even though the caller recovers.
/// - A user interrupt during the closure is an R error like any other and
///   also comes back as `Err`.
///
/// # Safety
/// Must be called on the R thread with R initialized. Every R value the
/// closure uses must be protected or reachable from a protected object.
pub(crate) unsafe fn top_level_exec<F, T>(closure: F) -> HarpResult<T>
where
    F: FnOnce() -> T + Copy,
    T: Copy,
{
    let lib = r_library()?;
    let mut payload = TopLevelPayload {
        closure: Some(closure),
        result: None,
    };
    // SAFETY: the trampoline only touches `payload`, which lives across the call.
    let completed = unsafe {
        (lib.r_toplevelexec)(
            Some(top_level_trampoline::<F, T>),
            &mut payload as *mut TopLevelPayload<F, T> as *mut std::ffi::c_void,
        )
    };
    match (completed, payload.result) {
        (0, _) | (_, None) => Err(HarpError::RError(arf_libr::RError::EvalError(
            "R signalled an error or was interrupted".to_string(),
        ))),
        (_, Some(result)) => Ok(result),
    }
}
