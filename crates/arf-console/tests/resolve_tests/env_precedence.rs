#[cfg(unix)]
use super::fixtures::FakeRigEnvironment;
use super::fixtures::isolated_command;
use super::normalize::{assert_descriptor_path, assert_selected_by_nullable_fields_are_present};
#[cfg(unix)]
use super::normalize::{assert_structured_resolve_error, normalize_descriptor};

#[test]
#[cfg(unix)]
fn resolve_environment_version_is_invalid_invocation() {
    let environment = FakeRigEnvironment::new();
    let output = environment
        .command()
        .args(["r", "resolve"])
        .env("ARF_R_VERSION", "3.99.99")
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
#[cfg(unix)]
fn resolve_environment_version_reports_environment_source() {
    let environment = FakeRigEnvironment::new();
    let output = environment
        .command()
        .args(["r", "resolve"])
        .env("ARF_R_VERSION", "4.4.2")
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["selected_by"]["kind"], "version_request");
    assert_selected_by_nullable_fields_are_present(&value);
    assert!(value["selected_by"]["requested_r_home"].is_null());
    assert_eq!(value["selected_by"]["requested_version"], "4.4.2");
    assert_eq!(
        value["selected_by"]["source"]["kind"],
        "environment_variable"
    );
    assert_eq!(value["selected_by"]["source"]["name"], "ARF_R_VERSION");
    insta::assert_snapshot!(
        "resolve_environment_version_reports_environment_source",
        normalize_descriptor(&value)
    );
}

#[test]
fn resolve_environment_source_reports_environment_origin() {
    let temp = tempfile::tempdir().unwrap();
    let output = isolated_command(temp.path())
        .args(["r", "resolve"])
        .env("ARF_R_HOME", temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["selected_by"]["kind"], "r_home");
    assert_selected_by_nullable_fields_are_present(&value);
    assert!(value["selected_by"]["requested_version"].is_null());
    assert_eq!(
        value["selected_by"]["source"]["kind"],
        "environment_variable"
    );
    assert_eq!(value["selected_by"]["source"]["name"], "ARF_R_HOME");
}

#[test]
fn resolve_cli_home_takes_precedence_over_conflicting_environment_sources() {
    let temp = tempfile::tempdir().unwrap();
    let r_home = temp.path().join("cli-r-home");
    let library = arf_libr::r_library_path(&r_home);
    std::fs::create_dir_all(library.parent().unwrap()).unwrap();
    std::fs::write(library, []).unwrap();

    let output = isolated_command(temp.path())
        .args(["r", "resolve", "--r-home"])
        .arg(&r_home)
        .env("ARF_R_HOME", "/env/R")
        .env("ARF_R_VERSION", "not a version")
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["selected_by"]["kind"], "r_home");
    assert_eq!(
        value["selected_by"]["source"]["kind"],
        "command_line_argument"
    );
    assert_eq!(value["selected_by"]["source"]["name"], "--r-home");
    assert_descriptor_path(&value, &value["target"]["r_home"], &r_home);
}

#[test]
fn resolve_conflicting_r_source_environment_values_fail() {
    let temp = tempfile::tempdir().unwrap();
    let output = isolated_command(temp.path())
        .args(["r", "resolve"])
        .env("ARF_R_HOME", "/env/R")
        .env("ARF_R_VERSION", "4.5")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("ARF_R_HOME and ARF_R_VERSION cannot be used together"),
        "stderr: {:?}",
        output.stderr
    );
}

#[test]
fn non_r_commands_ignore_conflicting_r_source_environment_values() {
    let temp = tempfile::tempdir().unwrap();
    let output = isolated_command(temp.path())
        .args(["completions", "bash"])
        .env("ARF_R_HOME", "/env/R")
        .env("ARF_R_VERSION", "4.5")
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
}

#[test]
fn cli_home_precedes_conflicting_environment_sources_for_r_consumers() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("arf.toml");
    std::fs::write(
        &config,
        r#"[startup]
r_source = "auto"
"#,
    )
    .unwrap();
    let script = temp.path().join("script.R");
    std::fs::write(&script, "1 + 1\n").unwrap();
    let missing_home = temp.path().join("missing-cli-home");

    let args_for = |prefix: &[&str], suffix: &[&str]| {
        prefix
            .iter()
            .copied()
            .chain(["--config"])
            .chain(std::iter::once(config.to_str().unwrap()))
            .chain(["--r-home"])
            .chain(std::iter::once(missing_home.to_str().unwrap()))
            .chain(suffix.iter().copied())
            .map(std::ffi::OsString::from)
            .collect::<Vec<_>>()
    };
    let cases = [
        ("REPL", args_for(&[], &[])),
        ("eval", args_for(&[], &["--eval", "1 + 1"])),
        ("file", args_for(&[], &["--file", script.to_str().unwrap()])),
        ("headless", args_for(&["headless"], &[])),
        ("r resolve", args_for(&["r", "resolve"], &[])),
    ];

    for (mode, args) in cases {
        let output = isolated_command(temp.path())
            .args(args)
            .env("ARF_R_HOME", "/env/R")
            .env("ARF_R_VERSION", "not a version")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "{mode} unexpectedly accepted a nonexistent explicit R_HOME"
        );
        assert!(
            !stderr.contains("ARF_R_HOME and ARF_R_VERSION cannot be used together"),
            "{mode} reported an environment conflict instead of using --r-home: {stderr}"
        );
        assert!(
            stderr.contains("missing-cli-home"),
            "{mode} did not report the selected CLI R_HOME: {stderr}"
        );
    }
}
