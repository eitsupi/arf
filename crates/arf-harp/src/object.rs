//! Safe wrapper around R's SEXP objects.

use crate::error::{HarpError, HarpResult};
use crate::protect::RProtect;
use arf_libr::{r_library, r_nil_value, ParseStatus, SexpType, SEXP};
use std::ffi::CString;

/// A safe wrapper around an R SEXP object.
///
/// This struct manages the lifetime and protection of R objects.
#[derive(Debug)]
pub struct RObject {
    sexp: SEXP,
    _protect: RProtect,
}

impl RObject {
    /// Create a new RObject from a raw SEXP.
    ///
    /// # Safety
    /// The caller must ensure that `sexp` is a valid R object.
    pub unsafe fn new(sexp: SEXP) -> Self {
        let mut protect = RProtect::new();
        // SAFETY: The caller guarantees sexp is valid, and we're inside an unsafe fn
        let sexp = unsafe { protect.protect(sexp) };
        RObject {
            sexp,
            _protect: protect,
        }
    }

    /// Get the raw SEXP pointer.
    pub fn sexp(&self) -> SEXP {
        self.sexp
    }

    /// Check if this object is R's NULL.
    pub fn is_null(&self) -> bool {
        if let Ok(nil) = r_nil_value() {
            self.sexp == nil
        } else {
            false
        }
    }

    /// Get the type of this R object.
    pub fn sexp_type(&self) -> HarpResult<SexpType> {
        let lib = r_library()?;
        let type_int = unsafe { (lib.rf_typeof)(self.sexp) };
        // Convert integer to SexpType
        match type_int as u32 {
            0 => Ok(SexpType::NilSxp),
            1 => Ok(SexpType::SymSxp),
            2 => Ok(SexpType::ListSxp),
            3 => Ok(SexpType::ClosSxp),
            4 => Ok(SexpType::EnvSxp),
            5 => Ok(SexpType::PromSxp),
            6 => Ok(SexpType::LangSxp),
            7 => Ok(SexpType::SpecialSxp),
            8 => Ok(SexpType::BuiltinSxp),
            9 => Ok(SexpType::CharSxp),
            10 => Ok(SexpType::LglSxp),
            13 => Ok(SexpType::IntSxp),
            14 => Ok(SexpType::RealSxp),
            15 => Ok(SexpType::CplxSxp),
            16 => Ok(SexpType::StrSxp),
            17 => Ok(SexpType::DotSxp),
            18 => Ok(SexpType::AnySxp),
            19 => Ok(SexpType::VecSxp),
            20 => Ok(SexpType::ExprSxp),
            21 => Ok(SexpType::BcodeSxp),
            22 => Ok(SexpType::ExtptrSxp),
            23 => Ok(SexpType::WeakrefSxp),
            24 => Ok(SexpType::RawSxp),
            25 => Ok(SexpType::S4Sxp),
            _ => Err(HarpError::TypeMismatch {
                expected: "known SEXP type".to_string(),
                actual: format!("unknown type {}", type_int),
            }),
        }
    }
}

/// Parse and evaluate an R expression string, printing visible results.
pub fn eval_string(code: &str) -> HarpResult<RObject> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        // Create R string from code
        let code_cstring =
            CString::new(code).map_err(|_| HarpError::TypeMismatch {
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

        // Get the number of expressions
        let n_expr = (lib.rf_length)(parsed);
        let global_env = *lib.r_globalenv;

        let mut last_result = r_nil_value()?;

        // Evaluate each expression and print visible results
        for i in 0..n_expr as isize {
            let expr = (lib.vector_elt)(parsed, i);

            // Use withVisible() to evaluate and get visibility information
            let with_visible_result = eval_with_visible(expr, global_env, &mut protect)?;

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
struct ParsePayload {
    code_sexp: SEXP,
    status: ParseStatus,
    result: Option<SEXP>,
}

/// Callback for R_ToplevelExec - parses the expression.
unsafe extern "C" fn parse_callback(payload: *mut std::ffi::c_void) {
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

/// Payload for R_ToplevelExec callback - evaluation.
struct EvalPayload {
    call: SEXP,
    env: SEXP,
    result: Option<SEXP>,
}

/// Callback for R_ToplevelExec - evaluates the expression.
unsafe extern "C" fn eval_callback(payload: *mut std::ffi::c_void) {
    let data = unsafe { &mut *(payload as *mut EvalPayload) };
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };
    // Evaluate the expression - if this throws an error, R_ToplevelExec will catch it
    let result = unsafe { (lib.rf_eval)(data.call, data.env) };
    data.result = Some(result);
}

/// Evaluate an expression and return the value along with visibility.
/// Uses base::withVisible() to determine if result should be printed.
/// Uses R_ToplevelExec for safe error handling.
unsafe fn eval_with_visible(
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
            call,
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

/// Install (intern) an R symbol by name.
unsafe fn install_symbol(name: &str) -> HarpResult<SEXP> {
    let lib = r_library()?;
    let name_cstring = CString::new(name).map_err(|_| HarpError::TypeMismatch {
        expected: "valid UTF-8".to_string(),
        actual: "string with null byte".to_string(),
    })?;
    // SAFETY: rf_install is safe to call with a valid C string
    unsafe { Ok((lib.rf_install)(name_cstring.as_ptr())) }
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
        let code_cstring =
            CString::new(code).map_err(|_| HarpError::TypeMismatch {
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

/// Deparse an R expression to a string.
///
/// Calls R's `deparse()` function to convert an expression back to source code.
/// The expression is wrapped in `quote()` to prevent evaluation.
///
/// # Safety
/// The caller must ensure that `expr` is a valid SEXP pointer.
pub unsafe fn deparse_to_string(expr: SEXP) -> HarpResult<String> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        // Build the call: deparse(quote(expr))
        // We wrap in quote() to prevent R from evaluating the expression
        let quote_sym = install_symbol("quote")?;
        let deparse_sym = install_symbol("deparse")?;

        // Build: quote(expr) - this will return expr unevaluated when evaluated
        let quoted_expr =
            protect.protect((lib.rf_lcons)(quote_sym, (lib.rf_lcons)(expr, r_nil_value()?)));

        // Build: deparse(quote(expr))
        let call = protect.protect((lib.rf_lcons)(
            deparse_sym,
            (lib.rf_lcons)(quoted_expr, r_nil_value()?),
        ));

        // Use R_ToplevelExec for safe error handling
        let mut payload = EvalPayload {
            call,
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
            call,
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
        let code_cstring =
            CString::new(code).map_err(|_| HarpError::TypeMismatch {
                expected: "valid UTF-8".to_string(),
                actual: "string with null byte".to_string(),
            })?;

        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        let mut status = ParseStatus::Null;
        let _ = (lib.r_parsevector)(code_sexp, -1, &mut status, r_nil_value()?);

        Ok(status != ParseStatus::Incomplete)
    }
}
