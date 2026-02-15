//! R help system integration.
//!
//! This module provides access to R's help database using `utils::hsearch_db()`.
//!
//! # Acknowledgment
//!
//! This implementation is inspired by the **felp** package by Atsushi Yasumoto (atusy):
//! - Repository: <https://github.com/atusy/felp>
//! - CRAN: <https://cran.r-project.org/package=felp>
//!
//! The approach of using `utils::hsearch_db()` to retrieve the help database
//! was learned from felp's `fuzzyhelp()` implementation.

use crate::error::{HarpError, HarpResult};
use crate::protect::RProtect;
use arf_libr::{ParseStatus, SEXP, r_library, r_nil_value};
use std::ffi::CString;

/// A help topic from R's help database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpTopic {
    /// Package name containing this help topic.
    pub package: String,
    /// Topic name (the alias used to access the help).
    pub topic: String,
    /// Title/description of the help topic.
    pub title: String,
    /// Type of help entry (e.g., "help", "vignette", "demo").
    pub entry_type: String,
}

impl HelpTopic {
    /// Format the topic as "package::topic" for display.
    pub fn qualified_name(&self) -> String {
        format!("{}::{}", self.package, self.topic)
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

/// Get R help topics from the help database.
///
/// This function calls `utils::hsearch_db()` to retrieve all available help topics
/// from installed packages. The result includes help pages, vignettes, and demos.
///
/// # Returns
///
/// A vector of `HelpTopic` structs containing package, topic, title, and type.
///
/// # Errors
///
/// Returns an error if R evaluation fails or if the help database is unavailable.
///
/// # Implementation Notes
///
/// This is inspired by the felp package's approach of using `hsearch_db()$Base`
/// to get the help database in a structured format.
pub fn get_help_topics() -> HarpResult<Vec<HelpTopic>> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        // R code to get help database
        // hsearch_db() returns a list with $Base containing the help index
        // We extract the relevant columns: Package, Topic, Title, Type
        let code = r#"
            tryCatch({
                db <- utils::hsearch_db()
                base <- db$Base
                if (is.null(base)) {
                    data.frame(
                        Package = character(0),
                        Topic = character(0),
                        Title = character(0),
                        Type = character(0),
                        stringsAsFactors = FALSE
                    )
                } else {
                    data.frame(
                        Package = base$Package,
                        Topic = base$Topic,
                        Title = base$Title,
                        Type = base$Type,
                        stringsAsFactors = FALSE
                    )
                }
            }, error = function(e) {
                data.frame(
                    Package = character(0),
                    Topic = character(0),
                    Title = character(0),
                    Type = character(0),
                    stringsAsFactors = FALSE
                )
            })
        "#;

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

        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Ok(vec![]);
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
            return Ok(vec![]);
        }

        let result = protect.protect(payload.result.unwrap());

        // Extract data from the data.frame
        // A data.frame in R is a list of columns
        extract_help_topics(result)
    }
}

/// Extract help topics from R data.frame SEXP.
unsafe fn extract_help_topics(df: SEXP) -> HarpResult<Vec<HelpTopic>> {
    let lib = r_library()?;

    unsafe {
        // Get the number of rows (length of first column)
        let n_cols = (lib.rf_length)(df);
        if n_cols < 4 {
            return Ok(vec![]);
        }

        // Get columns by index (0=Package, 1=Topic, 2=Title, 3=Type)
        let packages = (lib.vector_elt)(df, 0);
        let topics = (lib.vector_elt)(df, 1);
        let titles = (lib.vector_elt)(df, 2);
        let types = (lib.vector_elt)(df, 3);

        let n_rows = (lib.rf_length)(packages) as isize;
        let mut result = Vec::with_capacity(n_rows as usize);

        for i in 0..n_rows {
            let package = extract_string_at(packages, i)?;
            let topic = extract_string_at(topics, i)?;
            let title = extract_string_at(titles, i)?;
            let entry_type = extract_string_at(types, i)?;

            result.push(HelpTopic {
                package,
                topic,
                title,
                entry_type,
            });
        }

        Ok(result)
    }
}

/// Extract a string from a character vector at a given index.
unsafe fn extract_string_at(sexp: SEXP, index: isize) -> HarpResult<String> {
    let lib = r_library()?;

    unsafe {
        let elt = (lib.string_elt)(sexp, index);
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

/// Evaluate R code and return the result as an optional String.
///
/// This is an internal helper that handles the common pattern of:
/// parsing R code, evaluating it via `R_ToplevelExec`, and extracting
/// a character string result.
///
/// Returns `Ok(Some(text))` if evaluation produces a character result,
/// `Ok(None)` if the result is `NULL`, or `Err` on failure.
unsafe fn eval_r_to_string(code: &str) -> HarpResult<Option<String>> {
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

/// Get help text for a specific topic.
///
/// This retrieves the help content as plain text using `tools::Rd2txt()`,
/// bypassing R's pager system. This is important on Windows where R's
/// help() function may try to open a GUI window.
///
/// The approach is inspired by the felp package's `get_help()` function.
///
/// # Arguments
///
/// * `topic` - The help topic name
/// * `package` - Optional package name to look in
///
/// # Returns
///
/// The help text as a String, or an error if the topic is not found.
pub fn get_help_text(topic: &str, package: Option<&str>) -> HarpResult<String> {
    let code = if let Some(pkg) = package {
        format!(
            r#"local({{
    x <- utils::help("{topic}", package = "{pkg}", help_type = "text")
    paths <- as.character(x)
    if (length(paths) == 0) return(NULL)
    file <- paths[1L]
    pkgname <- basename(dirname(dirname(file)))
    paste(utils::capture.output(
        tools::Rd2txt(utils:::.getHelpFile(file), package = pkgname)
    ), collapse = "\n")
}})"#,
            topic = escape_r_string(topic),
            pkg = escape_r_string(pkg)
        )
    } else {
        format!(
            r#"local({{
    x <- utils::help("{topic}", help_type = "text")
    paths <- as.character(x)
    if (length(paths) == 0) return(NULL)
    file <- paths[1L]
    pkgname <- basename(dirname(dirname(file)))
    paste(utils::capture.output(
        tools::Rd2txt(utils:::.getHelpFile(file), package = pkgname)
    ), collapse = "\n")
}})"#,
            topic = escape_r_string(topic)
        )
    };

    unsafe {
        eval_r_to_string(&code)?.ok_or_else(|| {
            HarpError::RError(arf_libr::RError::EvalError(format!(
                "No help found for topic '{}'",
                topic
            )))
        })
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

/// Show help for a specific topic (legacy function).
///
/// This calls `get_help_text()` and prints the result to stdout.
/// For better control, use `get_help_text()` directly.
///
/// # Arguments
///
/// * `topic` - The help topic name
/// * `package` - Optional package name to look in
pub fn show_help(topic: &str, package: Option<&str>) -> HarpResult<()> {
    let text = get_help_text(topic, package)?;
    println!("{}", text);
    Ok(())
}

/// Escape a string for use in R code.
fn escape_r_string(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_help_topic_qualified_name() {
        let topic = HelpTopic {
            package: "base".to_string(),
            topic: "print".to_string(),
            title: "Print Values".to_string(),
            entry_type: "help".to_string(),
        };

        assert_eq!(topic.qualified_name(), "base::print");
    }

    #[test]
    fn test_escape_r_string() {
        assert_eq!(escape_r_string("hello"), "hello");
        assert_eq!(escape_r_string(r#"he"llo"#), r#"he\"llo"#);
        assert_eq!(escape_r_string("he\\llo"), "he\\\\llo");
    }
}
