//! R code formatter integration.
//!
//! This module provides formatting of R code using external formatter backends.

use crate::config::{FormatterBackend, ReprexFormatter};
use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

const AIR_STDIN_FILE_PATH: &str = "arf-reprex.R";

/// The user-facing context for a missing formatter diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatterUnavailableContext {
    /// An explicit `--reprex=format` request.
    ExplicitCli,
    /// A configured format mode that will fall back to reprex on mode.
    ConfiguredMode,
    /// The interactive `:reprex format` command.
    MetaCommand,
}

/// Build the diagnostic shown when a configured formatter is unavailable.
pub fn unavailable_message(
    formatter: ReprexFormatter,
    context: FormatterUnavailableContext,
) -> String {
    let backend = match formatter {
        ReprexFormatter::Auto => "Air and Arity formatter CLIs".to_string(),
        ReprexFormatter::Air => "Air CLI ('air' command)".to_string(),
        ReprexFormatter::Arity => "Arity CLI ('arity' command)".to_string(),
    };
    match context {
        FormatterUnavailableContext::ExplicitCli => format!(
            "Cannot use --reprex=format: {backend} not found in PATH.\n{}",
            install_guidance(formatter)
        ),
        FormatterUnavailableContext::ConfiguredMode => format!(
            "Warning: Reprex format mode is configured but {backend} was not found; using reprex on mode."
        ),
        FormatterUnavailableContext::MetaCommand => {
            format!("Error: Cannot use reprex format mode - {backend} not found in PATH.")
        }
    }
}

fn install_guidance(formatter: ReprexFormatter) -> String {
    match formatter {
        ReprexFormatter::Auto => {
            "Install Air CLI from https://github.com/posit-dev/air or Arity CLI from https://github.com/jolars/arity".to_string()
        }
        ReprexFormatter::Air => format!(
            "Install Air CLI from {}",
            FormatterBackend::Air.install_url()
        ),
        ReprexFormatter::Arity => format!(
            "Install Arity CLI from {}",
            FormatterBackend::Arity.install_url()
        ),
    }
}

/// Resolve a configured formatter selector to a concrete backend.
pub fn resolve_formatter(selector: ReprexFormatter) -> Option<FormatterBackend> {
    resolve_formatter_with(selector, is_formatter_available)
}

/// Resolve a formatter using an injected availability function.
///
/// The injected form keeps selector precedence deterministic in tests without
/// mutating process-global PATH state or the availability cache.
pub fn resolve_formatter_with(
    selector: ReprexFormatter,
    mut available: impl FnMut(FormatterBackend) -> bool,
) -> Option<FormatterBackend> {
    match selector {
        ReprexFormatter::Auto => [FormatterBackend::Air, FormatterBackend::Arity]
            .into_iter()
            .find(|backend| available(*backend)),
        ReprexFormatter::Air => available(FormatterBackend::Air).then_some(FormatterBackend::Air),
        ReprexFormatter::Arity => {
            available(FormatterBackend::Arity).then_some(FormatterBackend::Arity)
        }
    }
}

/// Check if a formatter backend is available on the system.
pub fn is_formatter_available(formatter: FormatterBackend) -> bool {
    match formatter {
        FormatterBackend::Air => {
            static AIR_AVAILABLE: OnceLock<bool> = OnceLock::new();
            *AIR_AVAILABLE.get_or_init(|| {
                Command::new(formatter.command())
                    .arg("--version")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            })
        }
        FormatterBackend::Arity => {
            static ARITY_AVAILABLE: OnceLock<bool> = OnceLock::new();
            *ARITY_AVAILABLE.get_or_init(|| {
                Command::new(formatter.command())
                    .arg("--version")
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            })
        }
    }
}

/// Format R code using the selected backend.
///
/// Returns the formatted code on success, or a formatting error.
/// Callers must not evaluate the unformatted code when an error is returned.
///
/// The current backend reads code from stdin and writes formatted code to stdout.
/// Its virtual path lets the backend discover project configuration from the cwd.
pub fn format_code(formatter: FormatterBackend, code: &str) -> Result<String, FormatterError> {
    match formatter {
        FormatterBackend::Air | FormatterBackend::Arity => format_backend(formatter, code),
    }
}

fn format_backend(formatter: FormatterBackend, code: &str) -> Result<String, FormatterError> {
    // Skip empty or whitespace-only input
    if code.trim().is_empty() {
        return Ok(code.to_string());
    }

    // Check if formatter is available
    if !is_formatter_available(formatter) {
        log::debug!(
            "Formatter '{}' not available, skipping format",
            formatter.command()
        );
        return Err(FormatterError::Io {
            formatter,
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("{} command is unavailable", formatter.command()),
            ),
        });
    }

    match format_via_stdin(formatter, code) {
        Ok(formatted) => Ok(formatted),
        Err(e) => {
            log::debug!("Formatting failed: {}", e);
            Err(e)
        }
    }
}

fn formatter_args(formatter: FormatterBackend, virtual_path: &Path) -> Vec<String> {
    match formatter {
        FormatterBackend::Air => vec![
            "format".to_string(),
            "--stdin-file-path".to_string(),
            virtual_path.display().to_string(),
            "--force".to_string(),
        ],
        FormatterBackend::Arity => vec!["format".to_string(), "-".to_string()],
    }
}

fn format_via_stdin(formatter: FormatterBackend, code: &str) -> Result<String, FormatterError> {
    run_formatter_command(
        formatter,
        OsStr::new(formatter.command()),
        Path::new(AIR_STDIN_FILE_PATH),
        code,
    )
}

fn run_formatter_command(
    formatter: FormatterBackend,
    command: &OsStr,
    virtual_path: &Path,
    code: &str,
) -> Result<String, FormatterError> {
    let args = formatter_args(formatter, virtual_path);
    let mut child = Command::new(command)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| FormatterError::Io { formatter, source })?;

    child
        .stdin
        .take()
        .ok_or_else(|| FormatterError::Io {
            formatter,
            source: std::io::Error::other("formatter stdin unavailable"),
        })?
        .write_all(code.as_bytes())
        .map_err(|source| FormatterError::Io { formatter, source })?;
    let output = child
        .wait_with_output()
        .map_err(|source| FormatterError::Io { formatter, source })?;

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
        return Err(FormatterError::FormatFailed {
            formatter,
            stderr: stderr.to_string(),
        });
    }

    let formatted =
        String::from_utf8(output.stdout).map_err(|error| FormatterError::FormatFailed {
            formatter,
            stderr: error.to_string(),
        })?;
    Ok(preserve_newline_style(code, formatted))
}

fn preserve_newline_style(original: &str, mut formatted: String) -> String {
    let original_crlf = original.ends_with("\r\n");
    let original_has_newline = original.ends_with('\n');
    let formatted_has_newline = formatted.ends_with('\n');

    if !original_has_newline && formatted_has_newline {
        formatted.pop();
        if formatted.ends_with('\r') {
            formatted.pop();
        }
    } else if original_crlf {
        formatted = formatted.replace("\r\n", "\n").replace('\n', "\r\n");
    } else if original_has_newline {
        formatted = formatted.replace("\r\n", "\n");
    }
    formatted
}

/// Errors that can occur during formatting.
#[derive(Debug)]
pub enum FormatterError {
    Unavailable {
        selector: ReprexFormatter,
    },
    Io {
        formatter: FormatterBackend,
        source: std::io::Error,
    },
    FormatFailed {
        formatter: FormatterBackend,
        stderr: String,
    },
}

impl std::fmt::Display for FormatterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FormatterError::Unavailable { selector } => write!(
                f,
                "No formatter backend is available for selector '{selector}'."
            ),
            FormatterError::Io { formatter, source } => write!(
                f,
                "{} formatting could not be started: {}\nEnsure {} CLI {} or later is installed.",
                formatter.display_name(),
                source,
                formatter.display_name(),
                formatter.minimum_version()
            ),
            FormatterError::FormatFailed { formatter, stderr } => write!(
                f,
                "{} formatting failed: {}\nEnsure {} CLI {} or later is installed.",
                formatter.display_name(),
                stderr.trim(),
                formatter.display_name(),
                formatter.minimum_version()
            ),
        }
    }
}

impl std::error::Error for FormatterError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_empty_code() {
        let result = format_code(FormatterBackend::Air, "").unwrap();
        assert_eq!(result, "");

        let result = format_code(FormatterBackend::Air, "   ").unwrap();
        assert_eq!(result, "   ");
    }

    #[test]
    fn formatter_args_use_air_stdin_contract() {
        assert_eq!(
            formatter_args(FormatterBackend::Air, Path::new("arf-reprex.R")),
            ["format", "--stdin-file-path", "arf-reprex.R", "--force"]
        );
    }

    #[test]
    fn formatter_args_use_arity_stdin_contract() {
        assert_eq!(
            formatter_args(FormatterBackend::Arity, Path::new("arf-reprex.R")),
            ["format", "-"]
        );
    }

    #[test]
    fn formatter_resolution_prefers_air_when_both_are_available() {
        assert_eq!(
            resolve_formatter_with(ReprexFormatter::Auto, |backend| {
                matches!(backend, FormatterBackend::Air | FormatterBackend::Arity)
            }),
            Some(FormatterBackend::Air)
        );
    }

    #[test]
    fn formatter_resolution_uses_air_when_only_air_is_available() {
        assert_eq!(
            resolve_formatter_with(ReprexFormatter::Auto, |backend| {
                backend == FormatterBackend::Air
            }),
            Some(FormatterBackend::Air)
        );
    }

    #[test]
    fn formatter_resolution_uses_arity_when_air_is_unavailable() {
        assert_eq!(
            resolve_formatter_with(ReprexFormatter::Auto, |backend| {
                backend == FormatterBackend::Arity
            }),
            Some(FormatterBackend::Arity)
        );
    }

    #[test]
    fn formatter_resolution_returns_none_when_no_backend_is_available() {
        assert_eq!(
            resolve_formatter_with(ReprexFormatter::Auto, |_| false),
            None
        );
    }

    #[test]
    fn explicit_formatter_selection_does_not_fallback() {
        assert_eq!(
            resolve_formatter_with(ReprexFormatter::Air, |backend| {
                backend == FormatterBackend::Arity
            }),
            None
        );
        assert_eq!(
            resolve_formatter_with(ReprexFormatter::Arity, |backend| {
                backend == FormatterBackend::Air
            }),
            None
        );
    }

    #[cfg(unix)]
    #[test]
    fn formatter_process_receives_stdin_and_expected_arguments() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let command = directory.path().join("fake-air");
        std::fs::write(
            &command,
            "#!/bin/sh\n[ \"$1\" = format ] || exit 10\n[ \"$2\" = --stdin-file-path ] || exit 11\n[ \"$3\" = virtual.R ] || exit 12\n[ \"$4\" = --force ] || exit 13\ncat\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&command, permissions).unwrap();

        let result = run_formatter_command(
            FormatterBackend::Air,
            command.as_os_str(),
            Path::new("virtual.R"),
            "x <- 1",
        );
        assert_eq!(result.unwrap(), "x <- 1");
    }

    #[cfg(unix)]
    #[test]
    fn arity_process_receives_stdin_and_expected_arguments() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let command = directory.path().join("fake-arity");
        std::fs::write(
            &command,
            "#!/bin/sh\n[ \"$1\" = format ] || exit 10\n[ \"$2\" = - ] || exit 11\n[ -z \"$3\" ] || exit 12\ncat\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&command, permissions).unwrap();

        let result = run_formatter_command(
            FormatterBackend::Arity,
            command.as_os_str(),
            Path::new("virtual.R"),
            "x <- 1",
        );
        assert_eq!(result.unwrap(), "x <- 1");
    }

    #[cfg(unix)]
    #[test]
    fn formatter_process_failure_is_returned_as_an_error() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let command = directory.path().join("fake-air");
        std::fs::write(&command, "#!/bin/sh\necho formatter failed >&2\nexit 17\n").unwrap();
        let mut permissions = std::fs::metadata(&command).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&command, permissions).unwrap();

        let result = run_formatter_command(
            FormatterBackend::Air,
            command.as_os_str(),
            Path::new("virtual.R"),
            "x <- 1",
        );
        assert!(
            matches!(result, Err(FormatterError::FormatFailed { stderr, .. }) if stderr.contains("formatter failed"))
        );
    }

    #[test]
    fn formatter_failure_message_includes_stderr_and_version_guidance() {
        let error = FormatterError::FormatFailed {
            formatter: FormatterBackend::Air,
            stderr: "stdin parse error\n".to_string(),
        };
        insta::assert_snapshot!(format!("{error}"), @r###"
Air formatting failed: stdin parse error
Ensure Air CLI 0.9.0 or later is installed.
"###);
    }

    #[test]
    fn arity_failure_message_includes_stderr_and_version_guidance() {
        let error = FormatterError::FormatFailed {
            formatter: FormatterBackend::Arity,
            stderr: "stdin parse error\n".to_string(),
        };
        insta::assert_snapshot!(format!("{error}"), @r###"
Arity formatting failed: stdin parse error
Ensure Arity CLI 0.18.0 or later is installed.
"###);
    }

    #[test]
    fn preserve_newline_style_keeps_trailing_style() {
        assert_eq!(preserve_newline_style("x", "x\n".to_string()), "x");
        assert_eq!(preserve_newline_style("x\n", "x\r\n".to_string()), "x\n");
        assert_eq!(preserve_newline_style("x\r\n", "x\n".to_string()), "x\r\n");
    }

    #[test]
    fn unavailable_explicit_cli_message_snapshot() {
        insta::assert_snapshot!(
            unavailable_message(
                ReprexFormatter::Air,
                FormatterUnavailableContext::ExplicitCli
            ),
            @r###"
Cannot use --reprex=format: Air CLI ('air' command) not found in PATH.
Install Air CLI from https://github.com/posit-dev/air
"###
        );
    }

    #[test]
    fn unavailable_configured_mode_message_snapshot() {
        insta::assert_snapshot!(
            unavailable_message(
                ReprexFormatter::Air,
                FormatterUnavailableContext::ConfiguredMode
            ),
            @r###"Warning: Reprex format mode is configured but Air CLI ('air' command) was not found; using reprex on mode."###
        );
    }

    #[test]
    fn unavailable_meta_command_message_snapshot() {
        insta::assert_snapshot!(
            unavailable_message(
                ReprexFormatter::Air,
                FormatterUnavailableContext::MetaCommand
            ),
            @r###"Error: Cannot use reprex format mode - Air CLI ('air' command) not found in PATH."###
        );
    }

    #[test]
    fn unavailable_arity_explicit_cli_message_snapshot() {
        insta::assert_snapshot!(
            unavailable_message(
                ReprexFormatter::Arity,
                FormatterUnavailableContext::ExplicitCli
            ),
            @r###"
Cannot use --reprex=format: Arity CLI ('arity' command) not found in PATH.
Install Arity CLI from https://github.com/jolars/arity
"###
        );
    }

    #[test]
    fn unavailable_auto_explicit_cli_message_snapshot() {
        insta::assert_snapshot!(
            unavailable_message(
                ReprexFormatter::Auto,
                FormatterUnavailableContext::ExplicitCli
            ),
            @r###"
Cannot use --reprex=format: Air and Arity formatter CLIs not found in PATH.
Install Air CLI from https://github.com/posit-dev/air or Arity CLI from https://github.com/jolars/arity
"###
        );
    }

    #[test]
    #[ignore] // Requires air to be installed
    fn test_format_simple_assignment() {
        let code = "x<-1+2";
        let result = format_code(FormatterBackend::Air, code).unwrap();
        assert_eq!(result, "x <- 1 + 2");
    }

    #[test]
    #[ignore] // Requires air to be installed
    fn test_format_function_definition() {
        let code = "f=function(x,y){x+y}";
        let result = format_code(FormatterBackend::Air, code).unwrap();
        // air formats this with proper spacing and indentation
        assert!(result.contains("function(x, y)"));
        assert!(result.contains("x + y"));
    }

    #[test]
    #[ignore] // Requires air to be installed
    fn test_format_preserves_trailing_newline_style() {
        // Without trailing newline
        let code = "x <- 1";
        let result = format_code(FormatterBackend::Air, code).unwrap();
        assert!(!result.ends_with('\n'));

        // With trailing newline
        let code = "x <- 1\n";
        let result = format_code(FormatterBackend::Air, code).unwrap();
        assert!(result.ends_with('\n'));
    }

    #[test]
    #[ignore] // Requires Arity >=0.18.0 to be installed
    fn test_arity_format_simple_assignment() {
        let code = "x<-1+2";
        let result = format_code(FormatterBackend::Arity, code).unwrap();
        assert_eq!(result, "x <- 1 + 2");
    }

    #[test]
    #[ignore] // Requires Arity >=0.18.0 to be installed
    fn test_arity_format_preserves_trailing_newline_style() {
        // Without trailing newline
        let code = "x <- 1";
        let result = format_code(FormatterBackend::Arity, code).unwrap();
        assert!(!result.ends_with('\n'));

        // With trailing newline
        let code = "x <- 1\n";
        let result = format_code(FormatterBackend::Arity, code).unwrap();
        assert!(result.ends_with('\n'));
    }
}
