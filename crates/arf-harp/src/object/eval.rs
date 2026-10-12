//! Evaluation of R code strings with visibility tracking.

use super::RObject;
use crate::error::{HarpError, HarpResult};
use crate::ffi::{EvalPayload, eval_callback, install_symbol};
use crate::protect::RProtect;
use arf_libr::{ParseStatus, SEXP, r_library, r_nil_value};
use std::ffi::CString;

/// Parse and evaluate an R expression string, printing visible results.
pub fn eval_string(code: &str) -> HarpResult<RObject> {
    let lib = r_library()?;
    let eval_env = unsafe { *lib.r_globalenv };
    eval_string_in_env(code, eval_env)
}

/// Parse and evaluate an R expression string in `R_BaseEnv`, printing visible results.
///
/// Evaluating in the base environment prevents user bindings in `.GlobalEnv` from
/// masking base functions. Use this for all internal infrastructure code that should
/// not be affected by the user's workspace.
pub fn eval_string_in_base(code: &str) -> HarpResult<RObject> {
    let lib = r_library()?;
    let eval_env = unsafe { *lib.r_baseenv };
    eval_string_in_env(code, eval_env)
}

fn eval_string_in_env(code: &str, eval_env: SEXP) -> HarpResult<RObject> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
            expected: "valid UTF-8".to_string(),
            actual: "string with null byte".to_string(),
        })?;

        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        // Parse the code using R_ToplevelExec for safe error handling
        let mut parse_payload = ParsePayload {
            code_sexp,
            status: ParseStatus::Null,
            result: None,
        };

        let parse_success = (lib.r_toplevelexec)(
            Some(parse_callback),
            &mut parse_payload as *mut ParsePayload as *mut std::ffi::c_void,
        );

        if parse_success == 0 || parse_payload.result.is_none() {
            return Err(HarpError::RError(arf_libr::RError::ParseError(
                "Parse error (R error during parsing)".to_string(),
            )));
        }

        let parsed = protect.protect(parse_payload.result.unwrap());

        match parse_payload.status {
            ParseStatus::Ok => {}
            ParseStatus::Incomplete => {
                return Err(HarpError::RError(arf_libr::RError::ParseError(
                    "Incomplete expression".to_string(),
                )));
            }
            ParseStatus::Error => {
                return Err(HarpError::RError(arf_libr::RError::ParseError(
                    "Parse error".to_string(),
                )));
            }
            _ => {
                return Err(HarpError::RError(arf_libr::RError::ParseError(format!(
                    "Unexpected parse status: {:?}",
                    parse_payload.status
                ))));
            }
        }

        let n_expr = (lib.rf_length)(parsed);
        let mut last_result = r_nil_value()?;

        for i in 0..n_expr as isize {
            let expr = (lib.vector_elt)(parsed, i);
            let with_visible_result = eval_with_visible(expr, eval_env, &mut protect)?;

            if let Some((value, visible)) = with_visible_result {
                last_result = value;
                if visible && value != r_nil_value()? {
                    (lib.rf_printvalue)(value);
                }
            }
        }

        Ok(RObject::new(last_result))
    }
}

/// Payload for R_ToplevelExec callback - parsing.
pub(super) struct ParsePayload {
    pub(super) code_sexp: SEXP,
    pub(super) status: ParseStatus,
    pub(super) result: Option<SEXP>,
}

/// Callback for R_ToplevelExec - parses the expression.
pub(super) unsafe extern "C" fn parse_callback(payload: *mut std::ffi::c_void) {
    let data = unsafe { &mut *(payload as *mut ParsePayload) };
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };
    let nil = match r_nil_value() {
        Ok(nil) => nil,
        Err(_) => return,
    };
    let result = unsafe { (lib.r_parsevector)(data.code_sexp, -1, &mut data.status, nil) };
    data.result = Some(result);
}

/// Evaluate an expression and return the value along with visibility.
/// Uses base::withVisible() to determine if result should be printed.
/// Uses R_ToplevelExec for safe error handling.
pub(super) unsafe fn eval_with_visible(
    expr: SEXP,
    env: SEXP,
    protect: &mut RProtect,
) -> HarpResult<Option<(SEXP, bool)>> {
    let lib = r_library()?;

    unsafe {
        // Build the call: withVisible(expr)
        // We need to use Rf_lcons to build a call
        let with_visible_sym = install_symbol("withVisible")?;
        let call = protect.protect((lib.rf_lcons)(
            with_visible_sym,
            (lib.rf_lcons)(expr, r_nil_value()?),
        ));

        // Use R_ToplevelExec for safe error handling
        let mut payload = EvalPayload {
            expr: call,
            env,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 || payload.result.is_none() {
            // Evaluation failed (R error occurred)
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "Evaluation error".to_string(),
            )));
        }

        let result = protect.protect(payload.result.unwrap());

        // withVisible returns a list with $value and $visible
        let value = (lib.vector_elt)(result, 0);
        let visible_sexp = (lib.vector_elt)(result, 1);

        // Get the visibility boolean
        // visible_sexp is a logical vector of length 1
        let visible = *(lib.logical)(visible_sexp) != 0;

        Ok(Some((value, visible)))
    }
}

/// Result of evaluating an R expression with visibility information.
#[derive(Debug)]
pub struct EvalResult {
    /// The result object.
    pub value: RObject,
    /// Whether the result should be printed (visible).
    pub visible: bool,
}

/// Parse and evaluate an R expression string, returning visibility information.
///
/// Unlike `eval_string`, this function does not print the result.
/// Use this for testing or when you need to control output yourself.
pub fn eval_string_with_visibility(code: &str) -> HarpResult<EvalResult> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        // Create R string from code
        let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
            expected: "valid UTF-8".to_string(),
            actual: "string with null byte".to_string(),
        })?;

        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        // Parse the code
        let mut status = ParseStatus::Null;
        let parsed = protect.protect((lib.r_parsevector)(
            code_sexp,
            -1,
            &mut status,
            r_nil_value()?,
        ));

        match status {
            ParseStatus::Ok => {}
            ParseStatus::Incomplete => {
                return Err(HarpError::RError(arf_libr::RError::ParseError(
                    "Incomplete expression".to_string(),
                )));
            }
            ParseStatus::Error => {
                return Err(HarpError::RError(arf_libr::RError::ParseError(
                    "Parse error".to_string(),
                )));
            }
            _ => {
                return Err(HarpError::RError(arf_libr::RError::ParseError(format!(
                    "Unexpected parse status: {:?}",
                    status
                ))));
            }
        }

        // Get the number of expressions
        let n_expr = (lib.rf_length)(parsed);
        let global_env = *lib.r_globalenv;

        let mut last_value = r_nil_value()?;
        let mut last_visible = false;

        // Evaluate each expression
        for i in 0..n_expr as isize {
            let expr = (lib.vector_elt)(parsed, i);

            // Use withVisible() to evaluate and get visibility information
            let with_visible_result = eval_with_visible(expr, global_env, &mut protect)?;

            if let Some((value, visible)) = with_visible_result {
                last_value = value;
                last_visible = visible && value != r_nil_value()?;
            }
        }

        Ok(EvalResult {
            value: RObject::new(last_value),
            visible: last_visible,
        })
    }
}
