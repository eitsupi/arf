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

#[test]
fn test_ipc_interrupt_accepts_pid_and_execution_id() {
    let sessions_dir = tempfile::tempdir().expect("Failed to create temp sessions dir");
    let output = sanitized_arf_command()
        .args(["ipc", "interrupt", "--pid", "12345", "--execution-id", "7"])
        .env("ARF_IPC_SESSIONS_DIR", sessions_dir.path())
        .output()
        .expect("Failed to run arf ipc interrupt");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("SESSION_NOT_FOUND") || stderr.contains("SESSION_NOT_FOUND"),
        "interrupt options should parse before session resolution: stdout={stdout}, stderr={stderr}"
    );
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("invalid value"),
        "interrupt CLI arguments should be accepted: {stderr}"
    );
}

#[test]
fn test_ipc_r_help_subcommands_accept_search_and_resolution_arguments() {
    let sessions_dir = tempfile::tempdir().expect("Failed to create temp sessions dir");

    for args in [
        vec![
            "ipc",
            "r-help",
            "search",
            "--query",
            "linear",
            "--package",
            "stats",
            "--limit",
            "20",
        ],
        vec![
            "ipc",
            "r-help",
            "show",
            "lm",
            "--package",
            "stats",
            "--current-package",
            "base",
            "--help-key",
            "lm",
        ],
    ] {
        let output = sanitized_arf_command()
            .args(args)
            .env("ARF_IPC_SESSIONS_DIR", sessions_dir.path())
            .output()
            .expect("Failed to run arf ipc help command");

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stdout.contains("SESSION_NOT_FOUND") || stderr.contains("SESSION_NOT_FOUND"),
            "help arguments should parse before session resolution: stdout={stdout}, stderr={stderr}"
        );
        assert!(
            !stderr.contains("unexpected argument") && !stderr.contains("invalid value"),
            "help arguments should be accepted: {stderr}"
        );
    }
}

#[test]
fn test_ipc_builtin_help_subcommand_remains_available() {
    let output = sanitized_arf_command()
        .args(["ipc", "help"])
        .output()
        .expect("Failed to run arf ipc help");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Commands:"));
    assert!(stdout.contains("r-help"));
}

#[test]
fn test_ipc_list_ignores_empty_arf_config() {
    let sessions_dir = tempfile::tempdir().expect("Failed to create temp sessions dir");

    let output = sanitized_arf_command()
        .args(["ipc", "list"])
        .env("ARF_CONFIG", "")
        .env("ARF_IPC_SESSIONS_DIR", sessions_dir.path())
        .output()
        .expect("Failed to run arf ipc list");

    assert!(
        output.status.success(),
        "ipc list should ignore empty ARF_CONFIG: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("stdout should be JSON: {error}\n{:?}", output.stdout));
    assert!(json["sessions"].is_array());
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

/// Check that Windows embedding reads the user Renviron exactly once, and
/// honors the options that suppress it before the Rstart params are applied.
#[cfg(windows)]
#[test]
fn test_windows_renviron_is_read_once_or_suppressed() {
    let fixture = tempfile::tempdir().expect("Failed to create Renviron fixture");
    let site_renviron_path = fixture.path().join("Renviron.site");
    let renviron_path = fixture.path().join("Renviron");
    std::fs::write(&site_renviron_path, "").expect("Failed to write site Renviron fixture");
    let project_library = fixture.path().join("project-library");
    let project_library = project_library.to_string_lossy().replace('\\', "/");
    std::fs::write(
        &renviron_path,
        r#"ARF_RENVIRON_READS="${ARF_RENVIRON_READS}x"
ARF_SAVED_R_LIBS_USER=${R_LIBS_USER}
R_LIBS_USER="${R_LIBS_USER};${ARF_TEST_PROJECT_LIBRARY}"
"#,
    )
    .expect("Failed to write Renviron fixture");

    let cases: [(&[&str], &str); 3] = [
        (&[], "normal startup"),
        (&["--no-environ"], "--no-environ"),
        (&["--vanilla"], "--vanilla"),
    ];

    for (flags, case_name) in cases {
        let mut command = sanitized_arf_command();
        command
            .env("R_ENVIRON", &site_renviron_path)
            .env("R_ENVIRON_USER", &renviron_path)
            .env_remove("ARF_RENVIRON_READS")
            .env_remove("ARF_SAVED_R_LIBS_USER")
            .env("R_LIBS_USER", "arf-original-user-library")
            .env("ARF_TEST_PROJECT_LIBRARY", &project_library)
            .args(flags)
            .args([
                "-e",
                r#"cat("ARF_RENVIRON_READS<", Sys.getenv("ARF_RENVIRON_READS"), ">\n", sep = ""); cat("ARF_SAVED_R_LIBS_USER<", Sys.getenv("ARF_SAVED_R_LIBS_USER"), ">\n", sep = ""); cat("R_LIBS_USER<", Sys.getenv("R_LIBS_USER"), ">\n", sep = "")"#,
            ]);

        let output = command.output().expect("Failed to run arf");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            output.status.success(),
            "{case_name} should succeed: stdout={stdout}, stderr={stderr}"
        );

        if flags.is_empty() {
            assert!(
                stdout.contains("ARF_RENVIRON_READS<x>"),
                "{case_name} should read Renviron once (expected marker x): stdout={stdout}, stderr={stderr}"
            );
            assert!(
                stdout.contains("ARF_SAVED_R_LIBS_USER<arf-original-user-library>"),
                "{case_name} should preserve the original R_LIBS_USER before project activation: stdout={stdout}, stderr={stderr}"
            );
            assert_eq!(
                stdout.matches(&project_library).count(),
                1,
                "{case_name} should apply the project library once: stdout={stdout}, stderr={stderr}"
            );
        } else {
            assert!(
                stdout.contains("ARF_RENVIRON_READS<>"),
                "{case_name} should not read Renviron: stdout={stdout}, stderr={stderr}"
            );
            assert!(
                stdout.contains("ARF_SAVED_R_LIBS_USER<>"),
                "{case_name} should leave the Renviron-only saved value unset: stdout={stdout}, stderr={stderr}"
            );
            assert!(
                !stdout.contains(&project_library),
                "{case_name} should not apply the project library: stdout={stdout}, stderr={stderr}"
            );
        }
    }
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
