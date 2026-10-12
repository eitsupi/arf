use super::*;

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

#[test]
fn explicit_reprex_format_failure_reports_pending_config_warning() {
    let empty_path = tempfile::tempdir().expect("Failed to create empty PATH directory");
    let mut config_file = NamedTempFile::new().expect("Failed to create temp config file");
    write!(
        config_file,
        r#"[history]
disabled = true

[reprex]
formatter = "air"
"#
    )
    .unwrap();

    let output = sanitized_arf_command()
        .env("PATH", empty_path.path())
        .env("RUST_LOG", "warn")
        .args([
            "--config",
            config_file.path().to_str().unwrap(),
            "--reprex",
            "format",
        ])
        .output()
        .expect("Failed to run arf with an unavailable formatter");

    assert!(
        !output.status.success(),
        "unavailable formatter should fail"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(stderr, @r###"
Warning: Config key history.disabled is deprecated; use history.mode = "volatile" instead.
Error: Cannot use --reprex=format: Air CLI ('air' command) not found in PATH.
Install Air CLI from https://github.com/posit-dev/air
"###);
}

#[cfg(unix)]
#[test]
fn test_script_startup_reports_deprecated_config_warning_once_after_reexec() {
    use std::env;

    let Ok(library) = arf_libr::find_r_library() else {
        return;
    };
    let library_dir = library.parent().expect("R library directory");
    let r_home = library_dir
        .parent()
        .expect("R_HOME directory")
        .to_path_buf();
    let current_library_path = env::var_os("LD_LIBRARY_PATH").unwrap_or_default();
    let paths = env::split_paths(&current_library_path)
        .filter(|path| path != library_dir)
        .collect::<Vec<_>>();
    let library_path = env::join_paths(paths).expect("valid LD_LIBRARY_PATH");
    assert!(
        !env::split_paths(&library_path).any(|path| path == library_dir),
        "test must force the loader re-exec"
    );

    let mut config_file = NamedTempFile::new().expect("Failed to create temp config file");
    write!(config_file, "[history]\ndisabled = true\n").unwrap();

    let output = sanitized_arf_command()
        .env("R_HOME", r_home)
        .env("LD_LIBRARY_PATH", library_path)
        .env("RUST_LOG", "warn")
        .args([
            "--config",
            config_file.path().to_str().unwrap(),
            "--vanilla",
            "-e",
            r#"quit(save = "no")"#,
        ])
        .output()
        .expect("Failed to run arf script mode");

    assert!(
        output.status.success(),
        "script should exit successfully: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(stderr, @r###"Warning: Config key history.disabled is deprecated; use history.mode = "volatile" instead."###);
}

#[test]
fn interactive_setup_failure_still_reports_config_warning() {
    let (config_file, _missing_r) = config_with_deprecated_history_and_missing_r();
    let output = sanitized_arf_command()
        .env("RUST_LOG", "warn")
        .args(["--config", config_file.path().to_str().unwrap()])
        .output()
        .expect("Failed to run interactive arf startup");

    assert_setup_failure_reports_config_warning_once(
        output,
        &_missing_r.path().join("missing-r-home"),
    );
}

#[test]
fn script_setup_failure_still_reports_config_warning() {
    let (config_file, _missing_r) = config_with_deprecated_history_and_missing_r();
    let output = sanitized_arf_command()
        .env("RUST_LOG", "warn")
        .args(["--config", config_file.path().to_str().unwrap(), "-e", "42"])
        .output()
        .expect("Failed to run arf script mode");

    assert_setup_failure_reports_config_warning_once(
        output,
        &_missing_r.path().join("missing-r-home"),
    );
}

#[test]
fn headless_setup_failure_still_reports_config_warning() {
    let (config_file, _missing_r) = config_with_deprecated_history_and_missing_r();
    let output = sanitized_arf_command()
        .env("RUST_LOG", "warn")
        .args(["headless", "--config", config_file.path().to_str().unwrap()])
        .output()
        .expect("Failed to run arf headless mode");

    assert_setup_failure_reports_config_warning_once(
        output,
        &_missing_r.path().join("missing-r-home"),
    );
}
