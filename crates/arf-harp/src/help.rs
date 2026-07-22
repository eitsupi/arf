//! R help system integration.
//!
//! This module provides access to installed package help indexes by reading
//! each package's help metadata files directly.
//!
//! # Acknowledgment
//!
//! This implementation is inspired by the **felp** package by Atsushi Yasumoto (atusy):
//! - Repository: <https://github.com/atusy/felp>
//! - CRAN: <https://cran.r-project.org/package=felp>
//!
//! The concept of searching the installed help database was learned from
//! felp's `fuzzyhelp()` implementation.

use crate::error::{HarpError, HarpResult};
use crate::lib_paths::{installed_package_dir, installed_package_dirs, lib_paths};
use crate::protect::RProtect;
use arf_libr::{ParseStatus, SEXP, r_library, r_nil_value};
use rd_helpdb::PackageHelpDb;
use rd_rds::{RObject, RValue, package::PackagesMatrix};
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

/// Get help, vignette, and demo topics from installed package metadata.
///
/// This function reads each installed package's help-search, vignette, and
/// demo indexes.
///
/// # Returns
///
/// A vector of `HelpTopic` structs containing package, topic, title, and type.
///
/// # Errors
///
/// Returns an error if R evaluation fails or if the help database is unavailable.
///
pub fn get_help_topics() -> HarpResult<Vec<HelpTopic>> {
    let mut topics = Vec::new();
    for (package, package_dir) in installed_package_dirs(&lib_paths()?) {
        let Ok(db) = PackageHelpDb::open(&package_dir) else {
            continue;
        };
        if let Ok(index) = db.search_index() {
            topics.extend(extract_help_topics(&index));
        }

        if let Ok(Some(index)) = db.vignettes() {
            topics.extend(index.entries().map(|entry| HelpTopic {
                package: package.clone(),
                topic: vignette_topic(entry),
                title: entry.title.clone(),
                entry_type: "vignette".to_string(),
            }));
        }

        if let Ok(Some(index)) = db.demos() {
            topics.extend(index.entries().map(|entry| HelpTopic {
                package: package.clone(),
                topic: entry.name.clone(),
                title: entry.title.clone(),
                entry_type: "demo".to_string(),
            }));
        }
    }
    Ok(topics)
}

// Mirror R's vignette-topic resolution from the index's filename fields.
fn vignette_topic(entry: &rd_helpdb::VignetteEntry) -> String {
    let (filename, from_file) = if !entry.r.is_empty() {
        (&entry.r, false)
    } else if !entry.pdf.is_empty() {
        (&entry.pdf, false)
    } else {
        (&entry.file, true)
    };
    let filename = if from_file {
        filename.rsplit(['/', '\\']).next().unwrap_or(filename)
    } else {
        filename
    };
    filename
        .rsplit_once('.')
        .map_or_else(|| filename.to_owned(), |(stem, _)| stem.to_owned())
}

#[cfg(test)]
mod vignette_tests {
    use super::vignette_topic;
    use rd_helpdb::VignetteEntry;

    fn entry(file: &str, pdf: &str, r: &str) -> VignetteEntry {
        VignetteEntry {
            file: file.to_owned(),
            title: String::new(),
            pdf: pdf.to_owned(),
            r: r.to_owned(),
            depends: Vec::new(),
            keywords: Vec::new(),
        }
    }

    #[test]
    fn resolves_vignette_topic_from_r_pdf_or_file() {
        assert_eq!(
            vignette_topic(&entry("ignored.Rmd", "ignored.pdf", "guide.R")),
            "guide"
        );
        assert_eq!(
            vignette_topic(&entry("ignored.Rmd", "guide.pdf", "")),
            "guide"
        );
        assert_eq!(
            vignette_topic(&entry("vignettes/guide.Rmd", "", "")),
            "guide"
        );
        assert_eq!(vignette_topic(&entry("guide", "", "")), "guide");
    }
}

fn extract_help_topics(index: &RObject) -> Vec<HelpTopic> {
    let RValue::List(items) = index.value() else {
        return Vec::new();
    };
    let Some(base) = items.first() else {
        return Vec::new();
    };
    let Ok(matrix) = PackagesMatrix::from_object(base) else {
        return Vec::new();
    };

    matrix
        .rows()
        .filter_map(|row| {
            let package = row.get("Package").and_then(|value| value)?;
            let topic = row.get("Topic").and_then(|value| value)?;
            let title = row.get("Title").flatten().unwrap_or("");
            Some(HelpTopic {
                package: package.to_owned(),
                topic: topic.to_owned(),
                title: title.to_owned(),
                entry_type: "help".to_string(),
            })
        })
        .collect()
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

/// Get help content as Markdown for a specific topic.
///
/// When `package` is known, this reads the installed package's compiled help
/// database directly. Without a package, it retains the R-evaluation-based
/// resolution needed for attached-package and search-path semantics.
///
/// # Arguments
///
/// * `topic` - The help topic name
/// * `package` - Optional package name to look in
///
/// # Returns
///
/// The help content as a Markdown string, or an error if the topic is not found.
pub fn get_help_markdown(topic: &str, package: Option<&str>) -> HarpResult<String> {
    match package {
        Some(package) => get_package_help_markdown(topic, package),
        None => get_help_markdown_via_r(topic),
    }
}

/// Get package help as Markdown without evaluating R for the help database.
///
/// The package directory is selected from the startup-cached library paths,
/// refreshed as needed by [`crate::lib_paths::lib_paths`].
///
/// `topic` is treated as an alias-or-exact-key input, with alias resolution
/// taking priority. This suits callers using display `Topic` values from
/// `Meta/hsearch.rds`.
pub fn get_package_help_markdown(topic: &str, package: &str) -> HarpResult<String> {
    let package_dir = installed_package_dir(&lib_paths()?, package).ok_or_else(|| {
        HarpError::PackageNotFound {
            package: package.to_string(),
        }
    })?;
    let db = PackageHelpDb::open(&package_dir).map_err(|source| HarpError::HelpDatabase {
        package: package.to_string(),
        topic: topic.to_string(),
        key: topic.to_string(),
        source: Box::new(source),
    })?;
    let resolved = db
        .resolve_alias(topic)
        .map_err(|source| HarpError::HelpDatabase {
            package: package.to_string(),
            topic: topic.to_string(),
            key: topic.to_string(),
            source: Box::new(source),
        })?;
    let key = resolved.unwrap_or(topic).to_string();
    let robj = db
        .raw_topic(&key)
        .map_err(|source| HarpError::HelpDatabase {
            package: package.to_string(),
            topic: topic.to_string(),
            key: key.clone(),
            source: Box::new(source),
        })?;
    let doc = rd_ast::lower_r_object(&robj).map_err(|source| HarpError::HelpLowering {
        package: package.to_string(),
        topic: topic.to_string(),
        key: key.clone(),
        source: Box::new(source),
    })?;
    let mut options = rd2qmd_core::RdConvertOptions::default();
    options.code.quarto_code_blocks = false;
    options.arguments_format = rd2qmd_core::ArgumentsFormat::PipeTable;
    Ok(rd2qmd_core::convert_rd_document(&doc, &options))
}

fn get_help_markdown_via_r(topic: &str) -> HarpResult<String> {
    let code = format!(
        r#"local({{
    x <- utils::help("{topic}", help_type = "text")
    paths <- as.character(x)
    if (length(paths) == 0) return(NULL)
    file <- paths[1L]
    rd <- utils:::.getHelpFile(file)
    paste0(as.character(rd, deparse = TRUE), collapse = "")
}})"#,
        topic = escape_r_string(topic)
    );

    let rd_content = unsafe {
        eval_r_to_string(&code)?.ok_or_else(|| {
            HarpError::RError(arf_libr::RError::EvalError(format!(
                "No help found for topic '{}'",
                topic
            )))
        })?
    };

    let parsed = rd_source::parse(rd_content.as_bytes()).map_err(|e| {
        HarpError::RError(arf_libr::RError::EvalError(format!(
            "Failed to parse Rd for Markdown conversion: {}",
            e
        )))
    })?;
    let mut options = rd2qmd_core::RdConvertOptions::default();
    options.code.quarto_code_blocks = false;
    options.arguments_format = rd2qmd_core::ArgumentsFormat::PipeTable;
    Ok(rd2qmd_core::convert_rd_document(
        parsed.document(),
        &options,
    ))
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

    fn matrix(values: Vec<rd_rds::RStr>, rows: i32, columns: &[&str]) -> RObject {
        let dimnames = vec![
            RObject::from_parts(
                RValue::Character(vec![rd_rds::RStr::Na; rows as usize]),
                rd_rds::Attributes::default(),
            ),
            RObject::from_parts(
                RValue::Character(columns.iter().map(|_| rd_rds::RStr::Na).collect()),
                rd_rds::Attributes::default(),
            ),
        ];
        RObject::from_parts(
            RValue::Character(values),
            rd_rds::Attributes::new(vec![
                rd_rds::Attribute::new(
                    rd_rds::Symbol::new("dim"),
                    RObject::from_parts(
                        RValue::Integer(vec![Some(rows), Some(columns.len() as i32)]),
                        rd_rds::Attributes::default(),
                    ),
                ),
                rd_rds::Attribute::new(
                    rd_rds::Symbol::new("dimnames"),
                    RObject::from_parts(RValue::List(dimnames), rd_rds::Attributes::default()),
                ),
            ]),
        )
    }

    #[test]
    fn malformed_and_na_help_indexes_are_skipped() {
        let columns = [
            "Package", "LibPath", "ID", "Name", "Title", "Topic", "Encoding",
        ];
        let base = matrix(vec![rd_rds::RStr::Na; columns.len()], 1, &columns);
        let index = RObject::from_parts(RValue::List(vec![base]), rd_rds::Attributes::default());

        assert!(extract_help_topics(&index).is_empty());
        assert!(
            extract_help_topics(&RObject::from_parts(
                RValue::List(Vec::new()),
                rd_rds::Attributes::default(),
            ))
            .is_empty()
        );
    }

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

    #[test]
    fn test_rd_conversion_strips_if_html_content() {
        // Regression test: `\if{html}{\out{...}}` blocks (e.g. asciicast
        // recordings) must not leak raw HTML into terminal help output.
        // Snapshotting the full output (rather than asserting individual
        // substrings) ensures any leaked tag, attribute, or inner text
        // shows up as a diff.
        let rd_content = r#"
\name{hello}
\title{Hello World}
\description{A simple function.}
\details{
Some details.
\if{html}{\out{<div class="asciicast"><span style="color: red;">colored</span></div>}}
More text after.
}
"#;

        let parsed = rd_source::parse(rd_content.as_bytes()).unwrap();
        let mut options = rd2qmd_core::RdConvertOptions::default();
        options.code.quarto_code_blocks = false;
        options.arguments_format = rd2qmd_core::ArgumentsFormat::PipeTable;
        let qmd = rd2qmd_core::convert_rd_document(parsed.document(), &options);

        insta::assert_snapshot!("rd_conversion_strips_if_html_content", qmd);
    }
}
