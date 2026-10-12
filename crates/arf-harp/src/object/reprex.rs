//! Reprex-style evaluation that echoes each expression before its result.

use super::RObject;
use super::eval::{ParsePayload, eval_with_visible, parse_callback};
use crate::error::{HarpError, HarpResult};
use crate::ffi::{EvalPayload, eval_callback, install_symbol};
use crate::protect::RProtect;
use arf_libr::{ParseStatus, SEXP, r_library, r_nil_value};
use std::ffi::CString;

/// Deparse an R expression to a string.
///
/// Calls R's `deparse()` function to convert an expression back to source code.
/// The expression is wrapped in `quote()` to prevent evaluation.
///
/// # Safety
/// The caller must ensure that `expr` is a valid SEXP pointer.
unsafe fn deparse_to_string(expr: SEXP) -> HarpResult<String> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        // Build the call: deparse(quote(expr))
        // We wrap in quote() to prevent R from evaluating the expression
        let quote_sym = install_symbol("quote")?;
        let deparse_sym = install_symbol("deparse")?;

        // Build: quote(expr) - this will return expr unevaluated when evaluated
        let quoted_expr = protect.protect((lib.rf_lcons)(
            quote_sym,
            (lib.rf_lcons)(expr, r_nil_value()?),
        ));

        // Build: deparse(quote(expr))
        let call = protect.protect((lib.rf_lcons)(
            deparse_sym,
            (lib.rf_lcons)(quoted_expr, r_nil_value()?),
        ));

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
                "deparse failed".to_string(),
            )));
        }

        let result = protect.protect(payload.result.unwrap());

        // deparse returns a character vector - join with newlines
        let n = (lib.rf_length)(result) as isize;
        let mut lines = Vec::with_capacity(n as usize);

        for i in 0..n {
            let elt = (lib.string_elt)(result, i);
            let c_str = (lib.r_charsxp)(elt);
            if !c_str.is_null() {
                let s = std::ffi::CStr::from_ptr(c_str)
                    .to_string_lossy()
                    .into_owned();
                lines.push(s);
            }
        }

        Ok(lines.join("\n"))
    }
}

/// Parse and evaluate R code in reprex mode, echoing source before each result.
///
/// For each expression in the code:
/// 1. Print the deparsed source code (without comment prefix)
/// 2. Evaluate the expression (output will have reprex prefix via libr callbacks)
///
/// This matches the standard reprex format:
/// ```text
/// 1 + 1
/// #> [1] 2
/// ```
///
pub fn eval_string_reprex(code: &str, comment: &str) -> HarpResult<RObject> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    // Enable reprex mode for R output
    arf_libr::set_reprex_mode(true, comment);

    unsafe {
        // Create R string from code
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
            arf_libr::set_reprex_mode(false, "");
            return Err(HarpError::RError(arf_libr::RError::ParseError(
                "Parse error (R error during parsing)".to_string(),
            )));
        }

        let parsed = protect.protect(parse_payload.result.unwrap());

        match parse_payload.status {
            ParseStatus::Ok => {}
            ParseStatus::Incomplete => {
                arf_libr::set_reprex_mode(false, "");
                return Err(HarpError::RError(arf_libr::RError::ParseError(
                    "Incomplete expression".to_string(),
                )));
            }
            ParseStatus::Error => {
                arf_libr::set_reprex_mode(false, "");
                return Err(HarpError::RError(arf_libr::RError::ParseError(
                    "Parse error".to_string(),
                )));
            }
            _ => {
                arf_libr::set_reprex_mode(false, "");
                return Err(HarpError::RError(arf_libr::RError::ParseError(format!(
                    "Unexpected parse status: {:?}",
                    parse_payload.status
                ))));
            }
        }

        // Get the number of expressions
        let n_expr = (lib.rf_length)(parsed);
        let global_env = *lib.r_globalenv;

        let mut last_result = r_nil_value()?;

        // Evaluate each expression, printing source first
        for i in 0..n_expr as isize {
            let expr = (lib.vector_elt)(parsed, i);

            // Deparse the expression to get source code
            match deparse_to_string(expr) {
                Ok(source) => {
                    // Print the source code (no comment prefix)
                    println!("{}", source);
                }
                Err(_) => {
                    // If deparse fails, skip printing source
                }
            }

            // Evaluate with visibility
            let with_visible_result = eval_with_visible(expr, global_env, &mut protect)?;

            if let Some((value, visible)) = with_visible_result {
                last_result = value;
                if visible && value != r_nil_value()? {
                    (lib.rf_printvalue)(value);
                }
            }

            // Flush any buffered reprex output
            arf_libr::flush_reprex_buffer();

            // Print a blank line between expressions for readability (if not last)
            if i < n_expr as isize - 1 {
                println!();
            }
        }

        // Disable reprex mode
        arf_libr::set_reprex_mode(false, "");

        Ok(RObject::new(last_result))
    }
}
