//! R code formatter integration.
//!
//! This module provides auto-formatting of R code using external formatters like `air`.
//!
//! # TODO
//! Currently uses a temp file workaround because `air format` doesn't support stdin/stdout.
//! See: <https://github.com/posit-dev/air/issues/202>
//!
//! When air adds stdin support (e.g., `echo "x<-1" | air format --stdin`), this module
//! should be updated to pipe directly to the formatter process, avoiding disk I/O overhead.

use std::io::Write;
use std::process::Command;
use std::sync::OnceLock;

/// The formatter command to use.
/// Currently only `air` is supported.
const FORMATTER_COMMAND: &str = "air";

/// Cached result of formatter availability check.
static FORMATTER_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Check if the air formatter is available on the system.
pub fn is_formatter_available() -> bool {
    *FORMATTER_AVAILABLE.get_or_init(|| {
        Command::new(FORMATTER_COMMAND)
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

/// Format R code using air.
///
/// Returns the formatted code on success, or the original code if formatting fails.
/// Errors are logged but not propagated to avoid disrupting the REPL flow.
///
/// # TODO
/// This implementation writes to a temp file because `air format` doesn't support stdin.
/// See: <https://github.com/posit-dev/air/issues/202>
///
/// Once air supports stdin, replace this with:
/// ```ignore
/// let output = Command::new("air")
///     .args(["format", "--stdin"])
///     .stdin(Stdio::piped())
///     .stdout(Stdio::piped())
///     .spawn()?;
/// output.stdin.write_all(code.as_bytes())?;
/// let formatted = String::from_utf8(output.wait_with_output()?.stdout)?;
/// ```
pub fn format_code(code: &str) -> String {
    // Skip empty or whitespace-only input
    if code.trim().is_empty() {
        return code.to_string();
    }

    // Check if formatter is available
    if !is_formatter_available() {
        log::debug!(
            "Formatter '{}' not available, skipping format",
            FORMATTER_COMMAND
        );
        return code.to_string();
    }

    // Create temp file for formatting
    // TODO: Replace with stdin pipe when air supports it (posit-dev/air#202)
    match format_via_temp_file(code) {
        Ok(formatted) => formatted,
        Err(e) => {
            log::debug!("Formatting failed: {}, using original code", e);
            code.to_string()
        }
    }
}

/// Format code by writing to a temp file and running the formatter.
///
/// This is a workaround for formatters that don't support stdin.
fn format_via_temp_file(code: &str) -> Result<String, FormatterError> {
    // Create a temp file with .R extension so the formatter recognizes it
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join("arf-format.R");

    // Write code to temp file
    let mut file = std::fs::File::create(&temp_path)?;
    file.write_all(code.as_bytes())?;
    file.flush()?;
    drop(file); // Ensure file is closed before formatter reads it

    // Run formatter
    let output = Command::new(FORMATTER_COMMAND)
        .arg("format")
        .arg(&temp_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Parse errors are common (incomplete expressions), don't log as error
        if stderr.contains("Parse") || stderr.contains("parse") {
            log::trace!(
                "Formatter parse error (expected for incomplete code): {}",
                stderr
            );
        } else {
            log::debug!("Formatter returned error: {}", stderr);
        }
        return Err(FormatterError::FormatFailed(stderr.to_string()));
    }

    // Read formatted code back
    let formatted = std::fs::read_to_string(&temp_path)?;

    // Clean up temp file (ignore errors)
    let _ = std::fs::remove_file(&temp_path);

    // air adds a trailing newline, but we want to preserve the original style
    // If the original didn't end with newline, strip the added one
    let formatted = if !code.ends_with('\n') && formatted.ends_with('\n') {
        formatted.trim_end_matches('\n').to_string()
    } else {
        formatted
    };

    Ok(formatted)
}

/// Errors that can occur during formatting.
#[derive(Debug)]
enum FormatterError {
    Io(std::io::Error),
    FormatFailed(String),
}

impl From<std::io::Error> for FormatterError {
    fn from(e: std::io::Error) -> Self {
        FormatterError::Io(e)
    }
}

impl std::fmt::Display for FormatterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatterError::Io(e) => write!(f, "I/O error: {}", e),
            FormatterError::FormatFailed(msg) => write!(f, "Format failed: {}", msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_empty_code() {
        let result = format_code("");
        assert_eq!(result, "");

        let result = format_code("   ");
        assert_eq!(result, "   ");
    }

    #[test]
    #[ignore] // Requires air to be installed
    fn test_format_simple_assignment() {
        let code = "x<-1+2";
        let result = format_code(code);
        assert_eq!(result, "x <- 1 + 2");
    }

    #[test]
    #[ignore] // Requires air to be installed
    fn test_format_function_definition() {
        let code = "f=function(x,y){x+y}";
        let result = format_code(code);
        // air formats this with proper spacing and indentation
        assert!(result.contains("function(x, y)"));
        assert!(result.contains("x + y"));
    }

    #[test]
    #[ignore] // Requires air to be installed
    fn test_format_preserves_trailing_newline_style() {
        // Without trailing newline
        let code = "x <- 1";
        let result = format_code(code);
        assert!(!result.ends_with('\n'));

        // With trailing newline
        let code = "x <- 1\n";
        let result = format_code(code);
        assert!(result.ends_with('\n'));
    }
}
