//! Output capture for the `evaluate` IPC method.
//!
//! Uses R's `sink()` + `textConnection` to capture stdout/stderr,
//! and `tryCatch(withVisible(...))` for value/error capture.
//! Output does not appear in the REPL.

use crate::ipc::protocol::EvaluateResult;

/// Evaluate R code with output capture, returning stdout, stderr, value, and error.
///
/// Runs on the R main thread (called from idle callback).
pub fn evaluate_with_capture(code: &str) -> EvaluateResult {
    let escaped = code.replace('\\', "\\\\").replace('\'', "\\'");

    let tmpfile = std::env::temp_dir().join(".arf_ipc_result.json");
    let tmppath = tmpfile.display().to_string().replace('\\', "/");

    // Single R expression that captures everything and writes JSON to a temp file.
    let capture_code = format!(
        r#"local({{
    .stdout_con <- textConnection(".arf_out", open = "w", local = TRUE)
    .stderr_con <- textConnection(".arf_err", open = "w", local = TRUE)
    sink(.stdout_con, type = "output")
    sink(.stderr_con, type = "message")
    .res <- tryCatch(
        withVisible(eval(parse(text = '{escaped}'), envir = globalenv())),
        error = function(e) list(value = NULL, visible = FALSE, error = conditionMessage(e))
    )
    sink(type = "message")
    sink(type = "output")
    close(.stderr_con)
    close(.stdout_con)
    .val <- if (!is.null(.res$error)) {{
        NULL
    }} else if (.res$visible) {{
        paste(utils::capture.output(print(.res$value)), collapse = "\n")
    }} else {{
        NULL
    }}
    .esc <- function(s) {{
        if (is.null(s)) return("null")
        s <- gsub("\\\\", "\\\\\\\\", s)
        s <- gsub("\"", "\\\\\"", s)
        s <- gsub("\n", "\\\\n", s)
        s <- gsub("\r", "\\\\r", s)
        s <- gsub("\t", "\\\\t", s)
        paste0("\"", s, "\"")
    }}
    writeLines(paste0(
        '{{"stdout":', .esc(paste(get(".arf_out"), collapse = "\n")),
        ',"stderr":', .esc(paste(get(".arf_err"), collapse = "\n")),
        ',"value":', .esc(.val),
        ',"error":', .esc(.res$error), '}}'
    ), '{tmppath}')
}})"#
    );

    match arf_harp::eval_string(&capture_code) {
        Ok(_) => match std::fs::read_to_string(&tmpfile) {
            Ok(json_str) => {
                let _ = std::fs::remove_file(&tmpfile);
                serde_json::from_str::<EvaluateResult>(&json_str).unwrap_or_else(|e| {
                    EvaluateResult {
                        stdout: String::new(),
                        stderr: String::new(),
                        value: None,
                        error: Some(format!("Failed to parse capture result: {e}")),
                    }
                })
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmpfile);
                EvaluateResult {
                    stdout: String::new(),
                    stderr: String::new(),
                    value: None,
                    error: Some(format!("Failed to read capture result: {e}")),
                }
            }
        },
        Err(e) => EvaluateResult {
            stdout: String::new(),
            stderr: String::new(),
            value: None,
            error: Some(format!("Failed to evaluate: {e}")),
        },
    }
}
