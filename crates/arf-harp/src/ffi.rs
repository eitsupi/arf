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
pub(crate) unsafe extern "C" fn eval_callback(payload: *mut std::ffi::c_void) {
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
