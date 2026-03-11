//! Output capture for the `evaluate` IPC method.
//!
//! Uses R's `sink()` + `textConnection` to capture stdout/stderr,
//! and `tryCatch(withVisible(...))` for value/error capture.
//! R writes raw text to a length-prefixed temp file; Rust reads it back
//! and constructs the JSON response. This avoids any JSON escaping in R.

use crate::ipc::protocol::EvaluateResult;
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counter for unique temp file names.
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a unique temp file path for this request.
fn unique_tmp_path() -> std::path::PathBuf {
    let pid = std::process::id();
    let seq = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(".arf_ipc_{pid}_{seq}.dat"))
}

/// Evaluate R code with output capture, returning stdout, stderr, value, and error.
///
/// Runs on the R main thread (called from idle callback).
///
/// Protocol: R writes a binary file with 4 length-prefixed fields:
///   `<header_line>\n<stdout><stderr><value><error>`
/// Header format: `stdout_len stderr_len value_len error_len`
/// A length of -1 means the field is NULL/absent.
pub fn evaluate_with_capture(code: &str) -> EvaluateResult {
    let escaped = code.replace('\\', "\\\\").replace('\'', "\\'");

    let tmpfile = unique_tmp_path();
    let tmppath = tmpfile.display().to_string().replace('\\', "/");

    // R captures output via sink/textConnection, then writes raw text with length header.
    // Rust handles all JSON construction — no escaping needed in R.
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
    .s_out <- paste(get(".arf_out"), collapse = "\n")
    .s_err <- paste(get(".arf_err"), collapse = "\n")
    .s_val <- if (!is.null(.res$error)) {{
        NULL
    }} else if (.res$visible) {{
        paste(utils::capture.output(print(.res$value)), collapse = "\n")
    }} else {{
        NULL
    }}
    .s_errmsg <- .res$error
    .header <- paste(
        nchar(.s_out, type = "bytes"),
        nchar(.s_err, type = "bytes"),
        if (is.null(.s_val)) -1L else nchar(.s_val, type = "bytes"),
        if (is.null(.s_errmsg)) -1L else nchar(.s_errmsg, type = "bytes")
    )
    .con <- file('{tmppath}', open = "wb")
    writeLines(.header, .con, sep = "\n")
    if (nchar(.s_out) > 0L) writeBin(charToRaw(.s_out), .con)
    if (nchar(.s_err) > 0L) writeBin(charToRaw(.s_err), .con)
    if (!is.null(.s_val) && nchar(.s_val) > 0L) writeBin(charToRaw(.s_val), .con)
    if (!is.null(.s_errmsg) && nchar(.s_errmsg) > 0L) writeBin(charToRaw(.s_errmsg), .con)
    close(.con)
}})"#
    );

    match arf_harp::eval_string(&capture_code) {
        Ok(_) => {
            let result = parse_capture_file(&tmpfile);
            let _ = std::fs::remove_file(&tmpfile);
            result
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmpfile);
            EvaluateResult {
                stdout: String::new(),
                stderr: String::new(),
                value: None,
                error: Some(format!("Failed to evaluate: {e}")),
            }
        }
    }
}

/// Parse the length-prefixed capture file into an EvaluateResult.
fn parse_capture_file(path: &std::path::Path) -> EvaluateResult {
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(e) => {
            return EvaluateResult {
                stdout: String::new(),
                stderr: String::new(),
                value: None,
                error: Some(format!("Failed to read capture file: {e}")),
            };
        }
    };

    // Find the header line (terminated by \n)
    let newline_pos = match data.iter().position(|&b| b == b'\n') {
        Some(p) => p,
        None => {
            return EvaluateResult {
                stdout: String::new(),
                stderr: String::new(),
                value: None,
                error: Some("Malformed capture file: no header".to_string()),
            };
        }
    };

    let header = String::from_utf8_lossy(&data[..newline_pos]);
    let lengths: Vec<i64> = header
        .split_whitespace()
        .filter_map(|s| s.parse().ok())
        .collect();

    if lengths.len() != 4 {
        return EvaluateResult {
            stdout: String::new(),
            stderr: String::new(),
            value: None,
            error: Some(format!("Malformed capture header: {header}")),
        };
    }

    let body = &data[newline_pos + 1..];
    let mut offset = 0usize;

    let read_field = |offset: &mut usize, len: i64| -> Option<String> {
        if len < 0 {
            return None;
        }
        let len = len as usize;
        if *offset + len > body.len() {
            return Some(String::new()); // Truncated, return empty
        }
        let s = String::from_utf8_lossy(&body[*offset..*offset + len]).into_owned();
        *offset += len;
        Some(s)
    };

    let stdout = read_field(&mut offset, lengths[0]).unwrap_or_default();
    let stderr = read_field(&mut offset, lengths[1]).unwrap_or_default();
    let value = read_field(&mut offset, lengths[2]);
    let error = read_field(&mut offset, lengths[3]);

    EvaluateResult {
        stdout,
        stderr,
        value,
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_parse_capture_file_basic() {
        let tmpdir = std::env::temp_dir();
        let path = tmpdir.join(".arf_test_capture.dat");

        // Simulate R output: stdout="hello", stderr="", value="[1] 42", error=NULL
        let stdout = b"hello";
        let value = b"[1] 42";
        let header = format!("{} 0 {} -1\n", stdout.len(), value.len());
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(header.as_bytes()).unwrap();
        file.write_all(stdout).unwrap();
        file.write_all(value).unwrap();
        drop(file);

        let result = parse_capture_file(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(result.stdout, "hello");
        assert_eq!(result.stderr, "");
        assert_eq!(result.value.as_deref(), Some("[1] 42"));
        assert!(result.error.is_none());
    }

    #[test]
    fn test_parse_capture_file_with_error() {
        let tmpdir = std::env::temp_dir();
        let path = tmpdir.join(".arf_test_capture_err.dat");

        let error_msg = b"object 'x' not found";
        let header = format!("0 0 -1 {}\n", error_msg.len());
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(header.as_bytes()).unwrap();
        file.write_all(error_msg).unwrap();
        drop(file);

        let result = parse_capture_file(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(result.stdout, "");
        assert_eq!(result.stderr, "");
        assert!(result.value.is_none());
        assert_eq!(result.error.as_deref(), Some("object 'x' not found"));
    }

    #[test]
    fn test_parse_capture_file_with_special_chars() {
        let tmpdir = std::env::temp_dir();
        let path = tmpdir.join(".arf_test_capture_special.dat");

        // Content with quotes, newlines, backslashes, and control chars
        let stdout = b"line1\nline2\t\"quoted\"\\\x08\x0c";
        let header = format!("{} 0 -1 -1\n", stdout.len());
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(header.as_bytes()).unwrap();
        file.write_all(stdout).unwrap();
        drop(file);

        let result = parse_capture_file(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(result.stdout, "line1\nline2\t\"quoted\"\\\x08\x0c");
        // Verify it serializes to valid JSON (the whole point of Rust-side construction)
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("line1\\nline2"));
    }

    #[test]
    fn test_unique_tmp_path() {
        let p1 = unique_tmp_path();
        let p2 = unique_tmp_path();
        assert_ne!(p1, p2);
    }
}
