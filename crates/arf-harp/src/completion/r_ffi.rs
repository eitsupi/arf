//! R FFI used by completion.

use crate::error::{HarpError, HarpResult};
use crate::protect::RProtect;
use arf_libr::{ParseStatus, SEXP, r_library, r_nil_value};
use std::ffi::CString;

/// Get the names from a package's namespace.
///
/// For `::` access (`triple_colon = false`), returns exported names via
/// `getNamespaceExports()`. For `:::` access (`triple_colon = true`),
/// returns all namespace objects (including internals) via
/// `ls(asNamespace(), all.names = TRUE)`.
///
/// Returns an empty vector if the R evaluation fails (e.g., package not
/// installed). May return `Err` if the R runtime itself is unavailable.
pub fn get_namespace_exports(pkg: &str, triple_colon: bool) -> HarpResult<Vec<String>> {
    let _guard = super::SuppressStderrGuard::new();

    let lib = r_library()?;
    let mut protect = RProtect::new();

    // For `:::`, list all namespace objects (including internals).
    // For `::`, only exported names.
    let code = if triple_colon {
        format!(
            r#"
            tryCatch({{
                ls(asNamespace("{pkg}"), all.names = TRUE)
            }}, error = function(e) character(0))
            "#,
            pkg = escape_r_string(pkg),
        )
    } else {
        format!(
            r#"
            tryCatch({{
                getNamespaceExports("{pkg}")
            }}, error = function(e) character(0))
            "#,
            pkg = escape_r_string(pkg),
        )
    };

    unsafe {
        let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
            expected: "string without null bytes".to_string(),
            actual: "string with null byte".to_string(),
        })?;

        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        let mut status = ParseStatus::Null;
        let parsed = protect.protect((lib.r_parsevector)(
            code_sexp,
            -1,
            &mut status,
            r_nil_value()?,
        ));

        if status != ParseStatus::Ok {
            return Ok(vec![]);
        }

        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Ok(vec![]);
        }

        let expr = (lib.vector_elt)(parsed, 0);
        let base_env = *lib.r_baseenv;

        let mut payload = EvalPayload {
            expr,
            env: base_env,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 || payload.result.is_none() {
            return Ok(vec![]);
        }

        let result = protect.protect(payload.result.unwrap());

        extract_string_vector(result)
    }
}

/// Get R's built-in completions using utils package functions.
///
/// # Arguments
/// * `line` - The input line
/// * `cursor_pos` - Cursor position in the line
/// * `timeout_ms` - Timeout in milliseconds (0 = no timeout)
///
/// Uses `base::setTimeLimit()` to prevent slow completions from blocking the UI.
/// This is similar to the approach used in radian.
pub(super) fn get_r_builtin_completions(
    line: &str,
    cursor_pos: usize,
    timeout_ms: u64,
) -> HarpResult<Vec<String>> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    // Convert timeout to seconds for R's setTimeLimit
    let timeout_secs = timeout_ms as f64 / 1000.0;
    let use_timeout = timeout_ms > 0;

    unsafe {
        // Build R code to call completion functions
        // Note: .guessTokenFromLine() must be called before .completeToken()
        // to set the token in .CompletionEnv
        //
        // When timeout is enabled, wrap completeToken() with setTimeLimit()
        // to prevent slow completions from blocking the UI.
        // Use both cpu and elapsed time limits for better coverage.
        // transient = TRUE makes the limit apply only to this expression.
        let code = format!(
            r#"
            local({{
                utils:::.assignLinebuffer("{line}")
                utils:::.assignEnd({cursor_pos}L)
                utils:::.guessTokenFromLine()
                tryCatch({{
                    if ({use_timeout}) base::setTimeLimit(cpu = {timeout}, elapsed = {timeout}, transient = TRUE)
                    utils:::.completeToken()
                    if ({use_timeout}) base::setTimeLimit(cpu = Inf, elapsed = Inf, transient = FALSE)
                    utils:::.retrieveCompletions()
                }}, error = function(e) {{
                    if ({use_timeout}) base::setTimeLimit(cpu = Inf, elapsed = Inf, transient = FALSE)
                    character(0)
                }})
            }})
            "#,
            line = escape_r_string(line),
            cursor_pos = cursor_pos,
            use_timeout = if use_timeout { "TRUE" } else { "FALSE" },
            timeout = timeout_secs,
        );

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

        if status != ParseStatus::Ok {
            return Ok(vec![]);
        }

        // Get the first expression
        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Ok(vec![]);
        }

        let expr = (lib.vector_elt)(parsed, 0);
        let base_env = *lib.r_baseenv;

        // Evaluate using R_ToplevelExec for safe error handling
        let mut payload = EvalPayload {
            expr,
            env: base_env,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 || payload.result.is_none() {
            return Ok(vec![]);
        }

        let result = protect.protect(payload.result.unwrap());

        // Convert result to Vec<String>
        extract_string_vector(result)
    }
}

/// Get the token being completed from the line.
pub fn get_token(line: &str, cursor_pos: usize) -> HarpResult<String> {
    // Suppress R console output during token extraction
    let _guard = super::SuppressStderrGuard::new();

    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        let code = format!(
            r#"
            local({{
                utils:::.assignLinebuffer("{}")
                utils:::.assignEnd({}L)
                utils:::.guessTokenFromLine()
            }})
            "#,
            escape_r_string(line),
            cursor_pos
        );

        let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
            expected: "valid UTF-8".to_string(),
            actual: "string with null byte".to_string(),
        })?;

        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        // Parse and evaluate
        let mut status = ParseStatus::Null;
        let parsed = protect.protect((lib.r_parsevector)(
            code_sexp,
            -1,
            &mut status,
            r_nil_value()?,
        ));

        if status != ParseStatus::Ok {
            return Ok(String::new());
        }

        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Ok(String::new());
        }

        let expr = (lib.vector_elt)(parsed, 0);
        let base_env = *lib.r_baseenv;

        let mut payload = EvalPayload {
            expr,
            env: base_env,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 || payload.result.is_none() {
            return Ok(String::new());
        }

        let result = protect.protect(payload.result.unwrap());

        // Extract single string
        extract_single_string(result)
    }
}

/// Payload for R_ToplevelExec callback.
struct EvalPayload {
    expr: SEXP,
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
    let result = unsafe { (lib.rf_eval)(data.expr, data.env) };
    data.result = Some(result);
}

/// Extract a character vector to Vec<String>.
unsafe fn extract_string_vector(sexp: SEXP) -> HarpResult<Vec<String>> {
    let lib = r_library()?;

    unsafe {
        // Check if it's a string vector
        if (lib.rf_isstring)(sexp) == 0 {
            return Ok(vec![]);
        }

        let len = (lib.rf_length)(sexp) as isize;
        let mut result = Vec::with_capacity(len as usize);

        for i in 0..len {
            let elt = (lib.string_elt)(sexp, i);
            let cstr = (lib.r_charsxp)(elt);
            if !cstr.is_null()
                && let Ok(s) = std::ffi::CStr::from_ptr(cstr).to_str()
            {
                result.push(s.to_string());
            }
        }

        Ok(result)
    }
}

/// Extract a single string from SEXP.
unsafe fn extract_single_string(sexp: SEXP) -> HarpResult<String> {
    let lib = r_library()?;

    unsafe {
        if (lib.rf_isstring)(sexp) == 0 || (lib.rf_length)(sexp) == 0 {
            return Ok(String::new());
        }

        let elt = (lib.string_elt)(sexp, 0);
        let cstr = (lib.r_charsxp)(elt);
        if cstr.is_null() {
            return Ok(String::new());
        }

        match std::ffi::CStr::from_ptr(cstr).to_str() {
            Ok(s) => Ok(s.to_string()),
            Err(_) => Ok(String::new()),
        }
    }
}

/// Escape a string for use in R code.
fn escape_r_string(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Check if the given names are functions in R.
///
/// This function checks multiple names efficiently in a single R call.
/// Returns a vector of booleans indicating whether each name is a function.
///
/// # Arguments
/// * `names` - The names to check
///
/// # Note
/// For performance, callers should limit the number of names checked.
/// Checking ~50 names takes <1ms, but thousands can take 100+ms.
pub fn check_if_functions(names: &[&str]) -> HarpResult<Vec<bool>> {
    if names.is_empty() {
        return Ok(vec![]);
    }

    // Suppress R console output during function checking
    let _guard = super::SuppressStderrGuard::new();

    let lib = r_library()?;
    let mut protect = RProtect::new();

    // Build R code to check all names at once
    // Using mode(get0(x, inherits=TRUE)) == "function" for each name
    let names_r: Vec<String> = names
        .iter()
        .map(|n| format!(r#""{}""#, escape_r_string(n)))
        .collect();
    let names_vector = names_r.join(", ");

    let code = format!(
        r#"
        local({{
            names <- c({})
            vapply(names, function(n) {{
                # Use eval(parse()) to handle both simple names and pkg::func syntax
                tryCatch({{
                    obj <- eval(parse(text = n))
                    is.function(obj)
                }}, error = function(e) FALSE)
            }}, logical(1), USE.NAMES = FALSE)
        }})
        "#,
        names_vector
    );

    unsafe {
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

        if status != ParseStatus::Ok {
            return Ok(vec![false; names.len()]);
        }

        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Ok(vec![false; names.len()]);
        }

        let expr = (lib.vector_elt)(parsed, 0);
        let global_env = *lib.r_globalenv;

        let mut payload = EvalPayload {
            expr,
            env: global_env,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 || payload.result.is_none() {
            return Ok(vec![false; names.len()]);
        }

        let result = protect.protect(payload.result.unwrap());

        // Extract logical vector
        extract_logical_vector(result, names.len())
    }
}

/// R's LGLSXP type code (logical vector).
const LGLSXP: i32 = 10;

/// Extract a logical vector to Vec<bool>.
unsafe fn extract_logical_vector(sexp: SEXP, expected_len: usize) -> HarpResult<Vec<bool>> {
    let lib = r_library()?;

    unsafe {
        // Check if it's a logical vector using TYPEOF
        if (lib.rf_typeof)(sexp) != LGLSXP {
            return Ok(vec![false; expected_len]);
        }

        let len = (lib.rf_length)(sexp) as usize;
        if len != expected_len {
            return Ok(vec![false; expected_len]);
        }

        let ptr = (lib.logical)(sexp);
        let mut result = Vec::with_capacity(len);

        for i in 0..len {
            // R's TRUE is 1, FALSE is 0, NA is INT_MIN
            let val = *ptr.add(i);
            result.push(val == 1);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_r_string() {
        assert_eq!(escape_r_string("hello"), "hello");
        assert_eq!(escape_r_string(r#"he"llo"#), r#"he\"llo"#);
        assert_eq!(escape_r_string("he\\llo"), "he\\\\llo");
        assert_eq!(escape_r_string("line1\nline2"), "line1\\nline2");
    }
}
