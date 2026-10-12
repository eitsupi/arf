//! Small R state queries.

use crate::error::{HarpError, HarpResult};
use crate::ffi::{EvalPayload, eval_callback, install_symbol};
use crate::protect::RProtect;
use arf_libr::{ParseStatus, r_library, r_nil_value};
use std::ffi::CString;

/// Get the number of frames on the R call stack.
///
/// This calls R's `sys.nframe()` function, which returns the number of
/// evaluation contexts (frames) on the R call stack.
///
/// This is useful for detecting whether R is at the top-level prompt
/// (n_frame == 0) or if some user code is requesting input, e.g., via
/// `readline()` or `menu()` (n_frame > 0).
///
/// Returns 0 when at top-level, > 0 when inside R function calls.
pub fn r_n_frame() -> HarpResult<i32> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        // Build the call: sys.nframe()
        let sys_nframe_sym = install_symbol("sys.nframe")?;
        let call = protect.protect((lib.rf_lcons)(sys_nframe_sym, r_nil_value()?));

        // Use R_ToplevelExec for safe error handling
        let mut payload = EvalPayload {
            expr: call,
            env: *lib.r_baseenv,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 || payload.result.is_none() {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "sys.nframe() failed".to_string(),
            )));
        }

        let result = payload.result.unwrap();

        // sys.nframe() returns an integer - get the first element
        let n_frame = *(lib.integer)(result);
        Ok(n_frame)
    }
}

/// Check if an expression is complete (for multiline input).
pub fn is_expression_complete(code: &str) -> HarpResult<bool> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
            expected: "valid UTF-8".to_string(),
            actual: "string with null byte".to_string(),
        })?;

        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        let mut status = ParseStatus::Null;
        let _ = (lib.r_parsevector)(code_sexp, -1, &mut status, r_nil_value()?);

        Ok(status != ParseStatus::Incomplete)
    }
}
