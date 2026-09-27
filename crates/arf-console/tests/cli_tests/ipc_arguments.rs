use super::*;

/// Test that `arf ipc eval` without a code argument reads from stdin.
/// The command fails at IPC (no running session) not at argument parsing.
#[test]
fn test_ipc_eval_stdin_fallback() {
    let sessions_dir = tempfile::tempdir().expect("Failed to create temp sessions dir");
    // Piped stdin is sufficient to make is_terminal() return false; no data
    // needs to be written since the process exits at session resolution first.
    let output = sanitized_arf_command()
        .args(["ipc", "eval"])
        .env("ARF_IPC_SESSIONS_DIR", sessions_dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to spawn arf");

    // Should fail at IPC level (no running session), not with "no code provided"
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stderr.contains("NO_CODE_PROVIDED"),
        "Should not complain about missing code when stdin is provided: {}",
        stderr
    );
    // Confirm failure reached IPC execution (SESSION_NOT_FOUND), not argument parsing
    assert!(
        stdout.contains("SESSION_NOT_FOUND") || stderr.contains("SESSION_NOT_FOUND"),
        "Should fail at IPC level with SESSION_NOT_FOUND: stdout={} stderr={}",
        stdout,
        stderr
    );
}

/// Test that `arf ipc send` without a code argument reads from stdin.
#[test]
fn test_ipc_send_stdin_fallback() {
    let sessions_dir = tempfile::tempdir().expect("Failed to create temp sessions dir");
    // Piped stdin is sufficient to make is_terminal() return false; no data
    // needs to be written since the process exits at session resolution first.
    let output = sanitized_arf_command()
        .args(["ipc", "send"])
        .env("ARF_IPC_SESSIONS_DIR", sessions_dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("Failed to spawn arf");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stderr.contains("NO_CODE_PROVIDED"),
        "Should not complain about missing code when stdin is provided: {}",
        stderr
    );
    // Confirm failure reached IPC execution (SESSION_NOT_FOUND), not argument parsing
    assert!(
        stdout.contains("SESSION_NOT_FOUND") || stderr.contains("SESSION_NOT_FOUND"),
        "Should fail at IPC level with SESSION_NOT_FOUND: stdout={} stderr={}",
        stdout,
        stderr
    );
}

/// Test that `arf -e` sources the user's `.Rprofile` during startup.
///
/// Verifies the full script-mode startup sequence: .Rprofile sourced →
/// side effect observable in output. This covers both Unix (R's built-in
/// profile loading via `setup_Rmainloop`) and Windows (manual
/// `source_r_profiles()` in `run_script()`), so it guards against
/// regressions on either path.
#[test]
fn test_eval_sources_user_rprofile() {
    let mut rprofile = NamedTempFile::new().expect("Failed to create temp .Rprofile");
    writeln!(rprofile, "cat('ARF_RPROFILE_MARKER\\n')").expect("Failed to write .Rprofile");
    let rprofile_path = rprofile.path().to_path_buf();

    let output = sanitized_arf_command()
        .env("R_PROFILE_USER", &rprofile_path)
        .args(["-e", "1"])
        .output()
        .expect("Failed to run arf -e");

    assert!(output.status.success(), "arf -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("ARF_RPROFILE_MARKER") || stderr.contains("ARF_RPROFILE_MARKER"),
        ".Rprofile should be sourced by arf -e. stdout={stdout}, stderr={stderr}"
    );
}

/// Test that an empty `R_PROFILE_USER` falls back to the working-directory
/// `.Rprofile` on Unix.
#[cfg(unix)]
#[test]
fn test_eval_empty_r_profile_user_sources_default_rprofile() {
    let working_dir = tempfile::tempdir().expect("Failed to create working directory");
    let rprofile_path = working_dir.path().join(".Rprofile");
    std::fs::write(&rprofile_path, "cat('ARF_RPROFILE_MARKER\\n')\n")
        .expect("Failed to write .Rprofile");

    let output = sanitized_arf_command()
        .current_dir(working_dir.path())
        .env("R_PROFILE_USER", "")
        .args(["-e", "1"])
        .output()
        .expect("Failed to run arf -e");

    assert!(output.status.success(), "arf -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("ARF_RPROFILE_MARKER") || stderr.contains("ARF_RPROFILE_MARKER"),
        "default .Rprofile should be sourced when R_PROFILE_USER is empty. \
         stdout={stdout}, stderr={stderr}"
    );
}

/// Test that `arf --vanilla -e` does NOT source the user's `.Rprofile`.
///
/// `--vanilla` implies `--no-init-file`, which must suppress .Rprofile
/// loading on both Unix and Windows startup paths.
#[test]
fn test_eval_vanilla_skips_user_rprofile() {
    let mut rprofile = NamedTempFile::new().expect("Failed to create temp .Rprofile");
    writeln!(rprofile, "cat('ARF_RPROFILE_MARKER\\n')").expect("Failed to write .Rprofile");
    let rprofile_path = rprofile.path().to_path_buf();

    let output = sanitized_arf_command()
        .env("R_PROFILE_USER", &rprofile_path)
        .args(["--vanilla", "-e", "1"])
        .output()
        .expect("Failed to run arf --vanilla -e");

    assert!(output.status.success(), "arf --vanilla -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stdout.contains("ARF_RPROFILE_MARKER") && !stderr.contains("ARF_RPROFILE_MARKER"),
        ".Rprofile must not be sourced under --vanilla. stdout={stdout}, stderr={stderr}"
    );
}

/// Test that positional script argument is rejected (regression test).
/// The old `arf file.R` syntax was removed; clap now rejects it as an unknown subcommand/argument.
#[test]
fn test_positional_script_rejected() {
    let output = sanitized_arf_command()
        .arg("some_script.R")
        .output()
        .expect("Failed to run arf");

    assert!(
        !output.status.success(),
        "positional script arg should be rejected"
    );

    assert_eq!(
        output.status.code(),
        Some(2),
        "positional script rejection should use clap's parse error exit code"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("some_script.R"),
        "Error should mention the offending token: {}",
        stderr
    );
}

/// Test that -e/--eval combined with a subcommand is rejected with exit code 2.
#[test]
fn test_eval_with_subcommand_rejected() {
    let output = sanitized_arf_command()
        .args(["-e", "1+1", "completions", "bash"])
        .output()
        .expect("Failed to run arf");

    assert!(
        !output.status.success(),
        "--eval with subcommand should be rejected"
    );

    assert_eq!(
        output.status.code(),
        Some(2),
        "--eval with subcommand should exit with code 2"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--eval")
            && stderr.contains("cannot be used")
            && stderr.contains("completions"),
        "Should show conflict error mentioning --eval and the subcommand: {}",
        stderr
    );
}

/// Test that -f/--file combined with a subcommand is rejected with exit code 2.
#[test]
fn test_file_with_subcommand_rejected() {
    let output = sanitized_arf_command()
        .args(["-f", "some.R", "completions", "bash"])
        .output()
        .expect("Failed to run arf");

    assert!(
        !output.status.success(),
        "--file with subcommand should be rejected"
    );

    assert_eq!(
        output.status.code(),
        Some(2),
        "--file with subcommand should exit with code 2"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--file")
            && stderr.contains("cannot be used")
            && stderr.contains("completions"),
        "Should show conflict error mentioning --file and the subcommand: {}",
        stderr
    );
}

// ============================================================================
// R Completion Tests (using R's internal completion functions)
// ============================================================================

/// Test that R's completion functions work.
#[test]
fn test_r_completion_functions() {
    // Test that utils completion functions are available
    let output = sanitized_arf_command()
        .args([
            "-e",
            r#"
            utils:::.assignLinebuffer("pri")
            utils:::.assignEnd(3)
            token <- utils:::.guessTokenFromLine()
            print(token)
        "#,
        ])
        .output()
        .expect("Failed to run arf -e");

    assert!(output.status.success(), "arf -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("pri"), "Token should be 'pri': {}", stdout);
}

/// Test that R's completeToken works.
#[test]
fn test_r_complete_token() {
    let output = sanitized_arf_command()
        .args([
            "-e",
            r#"
            utils:::.assignLinebuffer("prin")
            utils:::.assignEnd(4L)
            utils:::.guessTokenFromLine()
            utils:::.completeToken()
            comps <- utils:::.retrieveCompletions()
            print(comps)
        "#,
        ])
        .output()
        .expect("Failed to run arf -e");

    assert!(output.status.success(), "arf -e should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain "print" in completions
    assert!(
        stdout.contains("print"),
        "Completions should include 'print': {}",
        stdout
    );
}
