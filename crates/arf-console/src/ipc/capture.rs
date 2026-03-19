//! Output capture for the `evaluate` IPC method.
//!
//! Uses the WriteConsoleEx callback (`arf_libr::start_ipc_capture`) to capture
//! stdout/stderr, and `tryCatch(withVisible(...))` + `capture.output(print(...))`
//! for value/error capture. R writes value+error metadata to a temp file;
//! Rust reads it back and constructs the JSON response.

use crate::ipc::protocol::EvaluateResult;

/// Evaluate R code with output capture, returning stdout, stderr, value, and error.
///
/// Runs on the R main thread (called from idle callback).
///
/// stdout/stderr are captured via the WriteConsoleEx callback.
/// value and error are written to a temp file by R code.
///
/// Protocol: R writes a binary file with 2 length-prefixed fields:
///   `<header_line>\n<value><error>`
/// Header format: `value_len error_len`
/// A length of -1 means the field is NULL/absent.
pub fn evaluate_with_capture(code: &str) -> EvaluateResult {
    let escaped = code
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r");

    let tmpfile = tempfile::Builder::new()
        .prefix(".arf_ipc_")
        .suffix(".dat")
        .tempfile()
        .expect("Failed to create temp file for IPC capture");
    let tmppath = tmpfile.path().display().to_string().replace('\\', "/");

    // R code: tryCatch + withVisible for value/error, stdout/stderr via callback.
    let capture_code = format!(
        r#"local({{
    .res <- tryCatch(
        withVisible(eval(parse(text = '{escaped}'), envir = globalenv())),
        error = function(e) list(value = NULL, visible = FALSE, error = conditionMessage(e))
    )
    .s_val <- if (is.null(.res$error) && .res$visible) {{
        paste(utils::capture.output(print(.res$value)), collapse = "\n")
    }} else {{
        NULL
    }}
    .s_err <- .res$error
    .header <- paste(
        if (is.null(.s_val)) -1L else nchar(.s_val, type = "bytes"),
        if (is.null(.s_err)) -1L else nchar(.s_err, type = "bytes")
    )
    .con <- file('{tmppath}', open = "wb")
    writeLines(.header, .con, sep = "\n")
    if (!is.null(.s_val) && nchar(.s_val, type = "bytes") > 0L) writeBin(charToRaw(.s_val), .con)
    if (!is.null(.s_err) && nchar(.s_err, type = "bytes") > 0L) writeBin(charToRaw(.s_err), .con)
    close(.con)
}})"#
    );

    // Start capturing via WriteConsoleEx callback
    arf_libr::start_ipc_capture(false);

    let eval_result = arf_harp::eval_string(&capture_code);

    // Stop capturing and collect stdout/stderr
    let (stdout, stderr) = arf_libr::finish_ipc_capture();

    match eval_result {
        Ok(_) => {
            let mut result = parse_capture_file(tmpfile.path());
            // tmpfile is dropped automatically (and deleted) at scope end
            drop(tmpfile);
            result.stdout = stdout;
            result.stderr = stderr;
            result
        }
        Err(e) => {
            drop(tmpfile);
            EvaluateResult {
                stdout,
                stderr,
                value: None,
                error: Some(format!("Failed to evaluate: {e}")),
            }
        }
    }
}

/// Parse the capture file into an EvaluateResult (value + error only).
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

    if lengths.len() != 2 {
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

    let value = read_field(&mut offset, lengths[0]);
    let error = read_field(&mut offset, lengths[1]);

    EvaluateResult {
        stdout: String::new(),
        stderr: String::new(),
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

        // Simulate R output: value="[1] 42", error=NULL
        let value = b"[1] 42";
        let header = format!("{} -1\n", value.len());
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(header.as_bytes()).unwrap();
        file.write_all(value).unwrap();
        drop(file);

        let result = parse_capture_file(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(result.stdout, "");
        assert_eq!(result.stderr, "");
        assert_eq!(result.value.as_deref(), Some("[1] 42"));
        assert!(result.error.is_none());
    }

    #[test]
    fn test_parse_capture_file_with_error() {
        let tmpdir = std::env::temp_dir();
        let path = tmpdir.join(".arf_test_capture_err.dat");

        let error_msg = b"object 'x' not found";
        let header = format!("-1 {}\n", error_msg.len());
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
    fn test_parse_capture_file_with_value_and_error_none() {
        let tmpdir = std::env::temp_dir();
        let path = tmpdir.join(".arf_test_capture_val.dat");

        // value present, error absent
        let value = b"[1] \"hello\"";
        let header = format!("{} -1\n", value.len());
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(header.as_bytes()).unwrap();
        file.write_all(value).unwrap();
        drop(file);

        let result = parse_capture_file(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(result.value.as_deref(), Some("[1] \"hello\""));
        assert!(result.error.is_none());
        // Verify JSON serialization
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("hello"));
    }

    #[test]
    fn test_parse_capture_file_both_absent() {
        let tmpdir = std::env::temp_dir();
        let path = tmpdir.join(".arf_test_capture_empty.dat");

        // Both value and error absent (invisible result, no error)
        let header = "-1 -1\n";
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(header.as_bytes()).unwrap();
        drop(file);

        let result = parse_capture_file(&path);
        let _ = std::fs::remove_file(&path);

        assert!(result.value.is_none());
        assert!(result.error.is_none());
    }

    #[test]
    fn test_parse_capture_file_with_special_chars() {
        let tmpdir = std::env::temp_dir();
        let path = tmpdir.join(".arf_test_capture_special.dat");

        // Value containing quotes, newlines, backslashes, and control chars
        let value = b"[1] \"hello\\nworld\"\n\ttab\there\r\n\\backslash\\\x1b[31mred\x1b[0m";
        let header = format!("{} -1\n", value.len());
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(header.as_bytes()).unwrap();
        file.write_all(value).unwrap();
        drop(file);

        let result = parse_capture_file(&path);
        let _ = std::fs::remove_file(&path);

        let val = result.value.as_ref().expect("value should be present");
        assert!(result.error.is_none());

        // Verify value is preserved exactly
        assert_eq!(val.as_bytes(), value);

        // Verify JSON serialization works (special chars must be escaped properly)
        let json = serde_json::to_string(&result).unwrap();
        // Round-trip: deserialize back and check
        let deserialized: EvaluateResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.value.as_deref(), result.value.as_deref());
    }
}
