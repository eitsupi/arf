use super::*;
use std::path::Path;

/// Combined spinner test to avoid race conditions from parallel tests sharing global state.
/// Tests spinner lifecycle: config, start, stop, double-start, double-stop, and color.
#[test]
fn test_spinner_lifecycle() {
    // Verify the spinner lock isn't poisoned from a previous panic.
    // We release immediately since start_spinner/stop_spinner need to acquire it.
    drop(SPINNER_THREAD.lock().unwrap());

    // Reset to known state first
    stop_spinner();
    set_spinner_frames("");

    // Test 1: Initial state should be inactive
    assert!(!is_spinner_active());

    // Test 2: Spinner disabled with empty frames
    set_spinner_frames("");
    start_spinner();
    assert!(!is_spinner_active()); // Should not be active when frames are empty

    // Test 3: Basic start/stop
    set_spinner_frames("⠋⠙⠹");
    start_spinner();
    assert!(is_spinner_active());
    stop_spinner();
    assert!(!is_spinner_active());

    // Test 4: With color
    set_spinner_frames("⠋⠙⠹");
    set_spinner_color("\x1b[36m"); // Cyan
    start_spinner();
    assert!(is_spinner_active());
    stop_spinner();
    assert!(!is_spinner_active());

    // Test 5: Double start (should be no-op)
    set_spinner_frames("⠋⠙⠹");
    start_spinner();
    assert!(is_spinner_active());
    start_spinner(); // Second start should be a no-op
    assert!(is_spinner_active());
    stop_spinner();
    assert!(!is_spinner_active());

    // Test 6: Double stop (should be no-op)
    set_spinner_frames("⠋⠙⠹");
    start_spinner();
    stop_spinner();
    assert!(!is_spinner_active());
    stop_spinner(); // Second stop should be a no-op
    assert!(!is_spinner_active());

    // Cleanup
    set_spinner_frames("");
    set_spinner_color("");
}

/// Test command error state tracking.
///
/// These assertions are combined into a single test to avoid race conditions
/// when tests run in parallel (they share global state via CONDITION_ERROR_OCCURRED).
#[test]
fn test_command_error_state() {
    // Reset to known state first
    reset_command_error_state();

    // Initially no error
    assert!(!command_had_error(), "initial state should be false");

    // Mark an error condition
    mark_error_condition();
    assert!(command_had_error(), "should detect error after mark");

    // Reset should clear the error state
    reset_command_error_state();
    assert!(
        !command_had_error(),
        "should be false after reset_command_error_state"
    );

    // Mark error again and verify detection
    mark_error_condition();
    assert!(command_had_error(), "should detect error condition");

    // Final reset
    reset_command_error_state();
    assert!(!command_had_error(), "should be false after final reset");
}

#[test]
fn test_format_error_output() {
    // Basic error message
    let formatted = format_error_output("Error: foo");
    assert_eq!(formatted, "\x1b[31mError: foo\x1b[0m");

    // Empty string
    let formatted = format_error_output("");
    assert_eq!(formatted, "\x1b[31m\x1b[0m");

    // Multiline error
    let formatted = format_error_output("Error in x:\n  undefined");
    assert_eq!(formatted, "\x1b[31mError in x:\n  undefined\x1b[0m");
}

#[test]
fn test_strip_ansi_escapes() {
    // Strip red color codes
    let stripped = strip_ansi_escapes("\x1b[31mError: foo\x1b[0m");
    assert_eq!(stripped, "Error: foo");

    // Strip multiple color codes
    let stripped = strip_ansi_escapes("\x1b[1m\x1b[31mBold Red\x1b[0m");
    assert_eq!(stripped, "Bold Red");

    // No escape codes
    let stripped = strip_ansi_escapes("plain text");
    assert_eq!(stripped, "plain text");

    // Complex sequence (cursor movement)
    let stripped = strip_ansi_escapes("before\x1b[2Kafter");
    assert_eq!(stripped, "beforeafter");
}

#[test]
fn test_error_format_strip_roundtrip() {
    // Formatting and then stripping should give back original text
    let original = "Error: something went wrong";
    let formatted = format_error_output(original);
    let stripped = strip_ansi_escapes(&formatted);
    assert_eq!(stripped, original);
}

#[test]
fn test_strip_cr() {
    // CRLF should be converted to LF
    let stripped = strip_cr("Error: foo\r\nbar\r\n");
    assert_eq!(stripped, "Error: foo\nbar\n");

    // Text without CR should be unchanged (and borrowed, not owned)
    let input = "Error: foo\nbar\n";
    let stripped = strip_cr(input);
    assert_eq!(stripped, input);
    assert!(matches!(stripped, std::borrow::Cow::Borrowed(_)));

    // Standalone CR should also be stripped
    let stripped = strip_cr("Error: \"{\r\" の)");
    assert_eq!(stripped, "Error: \"{\" の)");

    // Mixed line endings: all CR should be removed
    let stripped = strip_cr("line1\r\nline2\nline3\r");
    assert_eq!(stripped, "line1\nline2\nline3");

    // Empty string
    let stripped = strip_cr("");
    assert_eq!(stripped, "");
    assert!(matches!(stripped, std::borrow::Cow::Borrowed(_)));
}

#[test]
fn test_parse_var_from_wrapper_script() {
    // Standard R wrapper format (unquoted)
    let script = "\
#!/bin/bash
R_HOME_DIR=/usr/lib64/R
R_SHARE_DIR=/usr/share/R
export R_SHARE_DIR
R_INCLUDE_DIR=/usr/include/R
export R_INCLUDE_DIR
R_DOC_DIR=/usr/share/doc/R
export R_DOC_DIR
";
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR"),
        Some("/usr/share/doc/R".to_string())
    );
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_SHARE_DIR"),
        Some("/usr/share/R".to_string())
    );
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_INCLUDE_DIR"),
        Some("/usr/include/R".to_string())
    );

    // Variable not present
    assert_eq!(parse_var_from_wrapper_script(script, "R_MISSING_VAR"), None);
}

#[test]
fn r_home_from_library_path_inverts_r_library_path() {
    let r_home = Path::new("/opt/R/4.5.2");
    assert_eq!(
        r_home_from_library_path(&r_library_path(r_home)),
        Some(r_home.to_path_buf())
    );
}

#[test]
fn r_home_from_library_path_rejects_other_files() {
    assert_eq!(
        r_home_from_library_path(Path::new("/opt/R/4.5.2/lib/R/not-libR")),
        None
    );
}

#[test]
fn r_home_from_rhome_output_extracts_path_after_warning() {
    let output = r"WARNING: ignoring environment value of R_HOME
/opt/R/4.0.5/lib/R
";
    assert_eq!(
        r_home_from_rhome_output(output),
        Some(r"/opt/R/4.0.5/lib/R".to_string())
    );
}

#[test]
fn r_home_from_rhome_output_handles_normal_output() {
    assert_eq!(
        r_home_from_rhome_output(
            r"/opt/R/4.0.5/lib/R
"
        ),
        Some(r"/opt/R/4.0.5/lib/R".to_string())
    );
}

#[test]
fn r_home_from_rhome_output_rejects_empty_output() {
    assert_eq!(r_home_from_rhome_output(""), None);
    assert_eq!(
        r_home_from_rhome_output(
            r" 
	 "
        ),
        None
    );
    assert_eq!(
        r_home_from_rhome_output(
            r"WARNING: ignoring environment value of R_HOME
"
        ),
        None
    );
}

#[test]
fn r_home_from_rhome_output_handles_missing_trailing_newline() {
    assert_eq!(
        r_home_from_rhome_output(r"/opt/R/4.0.5/lib/R"),
        Some(r"/opt/R/4.0.5/lib/R".to_string())
    );
}

#[test]
fn test_parse_var_from_wrapper_script_quoted() {
    // Single-quoted values
    let script = "R_DOC_DIR='/usr/share/doc/R'\nexport R_DOC_DIR\n";
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR"),
        Some("/usr/share/doc/R".to_string())
    );

    // Double-quoted values
    let script = "R_DOC_DIR=\"/usr/share/doc/R\"\nexport R_DOC_DIR\n";
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR"),
        Some("/usr/share/doc/R".to_string())
    );
}

#[test]
fn test_parse_var_from_wrapper_script_standard_install() {
    // Standard (non-Fedora) installation where paths are under R_HOME
    let script = "\
#!/bin/bash
R_HOME_DIR=/opt/R/4.5.2/lib/R
R_SHARE_DIR=/opt/R/4.5.2/lib/R/share
export R_SHARE_DIR
R_INCLUDE_DIR=/opt/R/4.5.2/lib/R/include
export R_INCLUDE_DIR
R_DOC_DIR=/opt/R/4.5.2/lib/R/doc
export R_DOC_DIR
";
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR"),
        Some("/opt/R/4.5.2/lib/R/doc".to_string())
    );
}

#[test]
fn test_parse_var_from_wrapper_script_empty_value() {
    let script = "R_DOC_DIR=\nexport R_DOC_DIR\n";
    assert_eq!(parse_var_from_wrapper_script(script, "R_DOC_DIR"), None);
}

#[test]
fn test_parse_var_from_wrapper_script_no_partial_prefix_match() {
    // R_DOC_DIR_EXTRA should NOT match when looking for R_DOC_DIR
    let script = "R_DOC_DIR_EXTRA=/some/path\nR_DOC_DIR=/usr/share/doc/R\n";
    assert_eq!(
        parse_var_from_wrapper_script(script, "R_DOC_DIR"),
        Some("/usr/share/doc/R".to_string())
    );

    // If only the longer name exists, R_DOC_DIR should not match
    let script = "R_DOC_DIR_EXTRA=/some/path\n";
    assert_eq!(parse_var_from_wrapper_script(script, "R_DOC_DIR"), None);
}

#[test]
fn test_set_r_path_vars_from_wrapper_skips_existing_env() {
    let _env_guard = super::test_utils::RDocDirGuard::new("/custom/doc");

    // Create a temp dir with a fake wrapper script (auto-cleaned on drop)
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    std::fs::write(
        bin_dir.join("R"),
        "R_DOC_DIR=/should/not/override\nexport R_DOC_DIR\n",
    )
    .unwrap();

    set_r_path_vars_from_wrapper(tmp.path());

    // R_DOC_DIR should NOT be overwritten
    assert_eq!(std::env::var("R_DOC_DIR").unwrap(), "/custom/doc");
}
