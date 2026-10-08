use std::io::Write;
use std::process::{Command, Output, Stdio};
use tempfile::NamedTempFile;

fn sanitized_arf_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_arf"));
    for variable in [
        "ARF_R_HOME",
        "ARF_R_VERSION",
        "ARF_HISTORY_DIR",
        "ARF_CONFIG",
    ] {
        command.env_remove(variable);
    }
    command
}

fn assert_top_level_scope_error(args: &[&str], expected: &[&str]) {
    let output = sanitized_arf_command()
        .args(args)
        .output()
        .expect("Failed to run arf");

    assert_eq!(
        output.status.code(),
        Some(2),
        "scope errors should use clap's exit code: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    for text in expected {
        assert!(
            stderr.contains(text),
            "scope error should contain {text:?}: {stderr}"
        );
    }
}

fn config_with_deprecated_history_and_missing_r() -> (NamedTempFile, tempfile::TempDir) {
    let missing_r = tempfile::tempdir().expect("Failed to create temp directory");
    let missing_r_home = missing_r.path().join("missing-r-home");
    let mut config_file = NamedTempFile::new().expect("Failed to create temp config file");
    write!(
        config_file,
        "[startup]\nr_source = {{ path = {:?} }}\n[history]\ndisabled = true\n",
        missing_r_home.to_string_lossy()
    )
    .expect("Failed to write temp config file");
    (config_file, missing_r)
}

fn assert_setup_failure_reports_config_warning_once(
    output: Output,
    missing_r_home: &std::path::Path,
) {
    assert!(!output.status.success(), "R setup should fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let normalized = stderr.replace(&missing_r_home.display().to_string(), "<missing-r-home>");
    insta::allow_duplicates! {
        insta::assert_snapshot!(normalized, @r###"
Warning: Config key history.disabled is deprecated; use history.mode = "volatile" instead.
Error: R_HOME path does not exist: <missing-r-home>
Check your r_source configuration.
"###);
    }
}

mod cli_surface;
mod config_history;
mod eval;
mod ipc_arguments;
mod scripts;
