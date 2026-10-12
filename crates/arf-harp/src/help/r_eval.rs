//! Help paths that evaluate R code.

use crate::error::{HarpError, HarpResult};
use crate::escape_r_string;
use crate::ffi::{EvalPayload, eval_callback};
use crate::protect::RProtect;
use arf_libr::{ParseStatus, r_library, r_nil_value};
use std::ffi::CString;

/// Evaluate R code and return the result as an optional String.
///
/// This shared helper handles the common pattern of:
/// parsing R code, evaluating it via `R_ToplevelExec`, and extracting
/// a character string result.
///
/// Returns `Ok(Some(text))` if evaluation produces a character result,
/// `Ok(None)` if the result is `NULL`, or `Err` on failure.
///
/// # Safety
///
/// R must already be initialized and the caller must ensure that evaluation
/// happens on R's thread.
pub unsafe fn eval_r_to_string(code: &str) -> HarpResult<Option<String>> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
        expected: "string without interior NUL bytes".to_string(),
        actual: "string containing interior NUL byte(s)".to_string(),
    })?;

    unsafe {
        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        let mut status = ParseStatus::Null;
        let parsed = protect.protect((lib.r_parsevector)(
            code_sexp,
            -1,
            &mut status,
            r_nil_value()?,
        ));

        if status != ParseStatus::Ok {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "Failed to parse R code".to_string(),
            )));
        }

        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "Empty R expression".to_string(),
            )));
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

        if success == 0 {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "R evaluation failed".to_string(),
            )));
        }

        let Some(result) = payload.result else {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "No result from R evaluation".to_string(),
            )));
        };

        if result == r_nil_value()? {
            return Ok(None);
        }

        // Check if it's a character vector (STRSXP = 16)
        let sexp_type = (lib.rf_typeof)(result);
        if sexp_type != 16 {
            return Err(HarpError::RError(arf_libr::RError::EvalError(
                "Unexpected result type from R".to_string(),
            )));
        }

        let len = (lib.rf_length)(result);
        if len == 0 {
            return Ok(None);
        }

        let str_elt = (lib.string_elt)(result, 0);
        let char_ptr = (lib.r_charsxp)(str_elt);
        if char_ptr.is_null() {
            return Ok(None);
        }

        let c_str = std::ffi::CStr::from_ptr(char_ptr);
        let text = c_str.to_string_lossy().into_owned();
        Ok(Some(text))
    }
}

/// Sentinel value returned by R when a vignette is in PDF format.
const PDF_VIGNETTE_SENTINEL: &str = "__PDF_VIGNETTE__";

/// Get vignette content as Markdown text.
///
/// This retrieves a vignette's HTML content via `utils::vignette()` and
/// converts it to Markdown using htmd. PDF vignettes cannot be displayed
/// in the terminal and will return an error with a descriptive message.
///
/// # Arguments
///
/// * `topic` - The vignette topic name
/// * `package` - The package name containing the vignette
///
/// # Returns
///
/// The vignette content as Markdown text, or an error if unavailable.
pub fn get_vignette_text(topic: &str, package: &str) -> HarpResult<String> {
    let code = format!(
        r#"local({{
    v <- tryCatch(
        utils::vignette("{topic}", package = "{pkg}"),
        error = function(e) NULL
    )
    if (is.null(v)) return(NULL)
    if (nchar(v$PDF) == 0) return(NULL)
    file <- file.path(v$Dir, "doc", v$PDF)
    if (!file.exists(file)) return(NULL)
    ext <- tolower(tools::file_ext(file))
    if (ext == "pdf") return("{sentinel}")
    paste(readLines(file, warn = FALSE), collapse = "\n")
}})"#,
        topic = escape_r_string(topic),
        pkg = escape_r_string(package),
        sentinel = escape_r_string(PDF_VIGNETTE_SENTINEL),
    );

    let html = unsafe {
        eval_r_to_string(&code)?.ok_or_else(|| {
            HarpError::RError(arf_libr::RError::EvalError(format!(
                "Vignette '{}' not found in package '{}'",
                topic, package
            )))
        })?
    };

    if html == PDF_VIGNETTE_SENTINEL {
        return Err(HarpError::RError(arf_libr::RError::EvalError(format!(
            r#"Vignette '{topic}' in package '{package}' is a PDF and cannot be displayed in the terminal.
Run in R: vignette("{topic}", package = "{package}")"#,
        ))));
    }

    r_vignette_to_md::convert(&html).map_err(|e| {
        HarpError::RError(arf_libr::RError::EvalError(format!(
            "Failed to convert vignette HTML: {}",
            e
        )))
    })
}
