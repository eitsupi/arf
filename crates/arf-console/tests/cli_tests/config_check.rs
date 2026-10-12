use super::*;

#[test]
fn test_config_check_reports_all_removed_reprex_keys() {
    let mut config_file = NamedTempFile::new().expect("Failed to create temp config file");
    write!(
        config_file,
        r##"[startup.mode]
reprex = true

[mode.reprex]
comment = "#> "

[reprex]
enabled = true
autoformat = true

[prompt.indicators]
autoformat = true
"##
    )
    .expect("Failed to write config file");

    let output = sanitized_arf_command()
        .args([
            "config",
            "check",
            "--config",
            config_file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run arf config check");

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(stderr, @r###"
Error: Config file has errors:

  [startup.mode] was removed; use [startup] reprex = "off"|"on"|"format"
  [mode.reprex] was removed; use [reprex]
  reprex.enabled was removed; use startup.reprex
  reprex.autoformat was removed; use startup.reprex
  prompt.indicators.autoformat was removed; use prompt.indicators.reprex_format
"###);
}

#[test]
fn test_config_check_uses_arf_config_and_cli_takes_precedence() {
    let env_config = NamedTempFile::new().expect("Failed to create env config file");
    let cli_config = NamedTempFile::new().expect("Failed to create CLI config file");

    let cases = [
        (env_config.path().as_os_str(), None, env_config.path()),
        (
            env_config.path().as_os_str(),
            Some(cli_config.path()),
            cli_config.path(),
        ),
        (
            std::ffi::OsStr::new(""),
            Some(cli_config.path()),
            cli_config.path(),
        ),
    ];

    for (env_path, cli_path, expected_path) in cases {
        let mut command = sanitized_arf_command();
        command
            .env("ARF_CONFIG", env_path)
            .args(["config", "check"]);
        if let Some(cli_path) = cli_path {
            command.arg("--config").arg(cli_path);
        }
        let output = command.output().expect("Failed to run arf config check");

        assert!(
            output.status.success(),
            "config check failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let expected_file = expected_path.file_name().unwrap().to_str().unwrap();
        assert!(String::from_utf8_lossy(&output.stdout).contains(expected_file));
    }
}

#[test]
fn test_empty_arf_config_fails_only_for_commands_that_consume_config() {
    let consumers: &[&[&str]] = &[
        &[],
        &["-e", "1"],
        &["headless"],
        &["r", "resolve"],
        &["history", "schema"],
        &["config", "check"],
    ];

    for args in consumers {
        let output = sanitized_arf_command()
            .env("ARF_CONFIG", "")
            .args(*args)
            .output()
            .unwrap_or_else(|error| panic!("Failed to run arf {args:?}: {error}"));

        assert_eq!(
            output.status.code(),
            Some(2),
            "empty ARF_CONFIG should fail before startup for {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("configuration file path must not be empty"),
            "expected empty-path clap error for {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty(), "unexpected stdout for {args:?}");
    }
}

#[test]
fn test_config_check_reports_malformed_arf_config() {
    let mut malformed_config = NamedTempFile::new().expect("Failed to create malformed config");
    write!(malformed_config, "not = [valid").unwrap();

    let malformed_output = sanitized_arf_command()
        .env("ARF_CONFIG", malformed_config.path())
        .args(["config", "check"])
        .output()
        .expect("Failed to check malformed ARF_CONFIG");
    assert_eq!(malformed_output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&malformed_output.stderr).contains("Config file has errors"));
}

#[test]
#[cfg(unix)]
fn test_config_init_ignores_arf_config() {
    let config_home = tempfile::tempdir().expect("Failed to create temporary config home");
    let expected_config = isolated_default_config_path(config_home.path());

    let output = sanitized_arf_command()
        .env("ARF_CONFIG", "")
        .env("HOME", config_home.path())
        .env("USERPROFILE", config_home.path())
        .env("APPDATA", config_home.path())
        .env("XDG_CONFIG_HOME", config_home.path())
        .args(["config", "init"])
        .output()
        .expect("Failed to run arf config init with empty ARF_CONFIG");

    assert!(
        output.status.success(),
        "config init should ignore ARF_CONFIG: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(expected_config.is_file());
    assert!(
        String::from_utf8_lossy(&output.stdout).contains(&expected_config.display().to_string())
    );
}

#[cfg(unix)]
fn isolated_default_config_path(home: &std::path::Path) -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    let config_dir = home.join("Library").join("Application Support");
    #[cfg(not(target_os = "macos"))]
    let config_dir = home.to_path_buf();
    config_dir.join("arf").join("arf.toml")
}

#[test]
fn test_completions_ignores_empty_arf_config() {
    let output = sanitized_arf_command()
        .env("ARF_CONFIG", "")
        .args(["completions", "zsh"])
        .output()
        .expect("Failed to run arf completions with empty ARF_CONFIG");

    assert!(
        output.status.success(),
        "completions should ignore ARF_CONFIG: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("#compdef arf"));
}

#[test]
fn test_config_check_reports_deprecated_history_keys_as_warnings() {
    let mut config_file = NamedTempFile::new().expect("Failed to create temp config file");
    write!(config_file, "[history]\ndisabled = true\n").unwrap();

    let output = sanitized_arf_command()
        .env("RUST_LOG", "warn")
        .args([
            "config",
            "check",
            "--config",
            config_file.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run arf config check");

    assert!(
        output.status.success(),
        "valid deprecated config should pass: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Config file is valid."));
    let stderr = String::from_utf8_lossy(&output.stderr);
    insta::assert_snapshot!(stderr, @r###"Warning: Config key history.disabled is deprecated; use history.mode = "volatile" instead."###);
}
