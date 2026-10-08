use super::*;

#[test]
fn test_r_source_flag_before_headless_rejected_with_corrected_form() {
    assert_top_level_scope_error(
        &["--r-home", "/tmp", "headless"],
        &[
            "--r-home",
            "before the 'headless' subcommand",
            "arf headless --r-home <R_HOME>",
        ],
    );
}

#[test]
fn test_no_banner_before_completions_rejected() {
    assert_top_level_scope_error(
        &["--no-banner", "completions", "bash"],
        &[
            "--no-banner",
            "not used by the 'completions' subcommand",
            "arf --no-banner",
        ],
    );
}

#[test]
fn test_top_level_config_before_history_schema_uses_configured_directory() {
    let history_dir = tempfile::tempdir().expect("Failed to create history directory");
    let mut config = NamedTempFile::new().expect("Failed to create config file");
    writeln!(
        config.as_file_mut(),
        "[history]\nmode = {{ dir = {:?} }}",
        history_dir.path().display().to_string()
    )
    .expect("Failed to write config file");

    let output = sanitized_arf_command()
        .arg("--config")
        .arg(config.path())
        .args(["history", "schema"])
        .output()
        .expect("Failed to run arf history schema");

    assert!(
        output.status.success(),
        "history schema with explicit config failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&history_dir.path().join("r.db").display().to_string()));
    assert!(stdout.contains(&history_dir.path().join("shell.db").display().to_string()));
}

#[test]
fn test_top_level_history_dir_before_history_schema_uses_explicit_directory() {
    let history_dir = tempfile::tempdir().expect("Failed to create history directory");
    let output = sanitized_arf_command()
        .arg("--history-dir")
        .arg(history_dir.path())
        .args(["history", "schema"])
        .output()
        .expect("Failed to run arf history schema");

    assert!(
        output.status.success(),
        "history schema with explicit directory failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&history_dir.path().join("r.db").display().to_string()));
    assert!(stdout.contains(&history_dir.path().join("shell.db").display().to_string()));
}

#[test]
fn test_no_history_before_history_schema_is_rejected_as_interactive_only() {
    assert_top_level_scope_error(
        &["--no-history", "history", "schema"],
        &[
            "--no-history",
            "not used by the 'history schema' subcommand",
            "arf --no-history",
        ],
    );
}

#[test]
fn test_top_level_config_before_config_check_rejected_with_nested_corrected_form() {
    let output = sanitized_arf_command()
        .args(["--config", "x", "config", "check"])
        .output()
        .expect("Failed to run arf config check");

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Usage: arf config check")
            && stderr.contains("before the 'config check' subcommand"),
        "{stderr}"
    );
    assert!(
        stderr.contains("arf config check --config <CONFIG>"),
        "{stderr}"
    );
    assert!(!stderr.contains("arf --config"), "{stderr}");
}

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

    let env_output = sanitized_arf_command()
        .env("ARF_CONFIG", env_config.path())
        .args(["config", "check"])
        .output()
        .expect("Failed to run arf config check with ARF_CONFIG");

    assert!(
        env_output.status.success(),
        "config check with ARF_CONFIG failed: {}",
        String::from_utf8_lossy(&env_output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&env_output.stdout)
            .contains(env_config.path().file_name().unwrap().to_str().unwrap())
    );

    let cli_with_valid_env_output = sanitized_arf_command()
        .env("ARF_CONFIG", env_config.path())
        .args(["config", "check", "--config"])
        .arg(cli_config.path())
        .output()
        .expect("Failed to run arf config check with CLI and environment paths");

    assert!(
        cli_with_valid_env_output.status.success(),
        "CLI config check should override ARF_CONFIG: {}",
        String::from_utf8_lossy(&cli_with_valid_env_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&cli_with_valid_env_output.stdout);
    assert!(stdout.contains(cli_config.path().file_name().unwrap().to_str().unwrap()));
    assert!(!stdout.contains(env_config.path().file_name().unwrap().to_str().unwrap()));

    let cli_output = sanitized_arf_command()
        .env("ARF_CONFIG", "")
        .args(["config", "check", "--config"])
        .arg(cli_config.path())
        .output()
        .expect("Failed to run arf config check with --config");

    assert!(
        cli_output.status.success(),
        "config check with --config failed: {}",
        String::from_utf8_lossy(&cli_output.stderr)
    );
    let stdout = String::from_utf8_lossy(&cli_output.stdout);
    assert!(stdout.contains(cli_config.path().file_name().unwrap().to_str().unwrap()));
    assert!(!stdout.contains(env_config.path().file_name().unwrap().to_str().unwrap()));
}

#[cfg(unix)]
#[test]
fn test_arf_config_replaces_instead_of_merging_default_config() {
    let env_config = NamedTempFile::new().expect("Failed to create env config file");
    let config_home = tempfile::tempdir().expect("Failed to create temporary config home");
    let default_config = isolated_default_config_path(config_home.path());
    std::fs::create_dir_all(default_config.parent().unwrap()).unwrap();
    std::fs::write(&default_config, "not = [valid").unwrap();

    let output = sanitized_arf_command()
        .env("ARF_CONFIG", env_config.path())
        .env("HOME", config_home.path())
        .env("XDG_CONFIG_HOME", config_home.path())
        .args(["config", "check"])
        .output()
        .expect("Failed to check ARF_CONFIG alongside an invalid default config");

    assert!(
        output.status.success(),
        "ARF_CONFIG should replace the default file: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_config_check_rejects_empty_arf_config() {
    let output = sanitized_arf_command()
        .env("ARF_CONFIG", "")
        .args(["config", "check"])
        .output()
        .expect("Failed to run arf config check with empty ARF_CONFIG");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("configuration file path must not be empty")
    );
    assert!(output.stdout.is_empty());
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
fn test_arf_config_does_not_mask_top_level_cli_scope_errors() {
    let config_path = std::ffi::OsStr::new("/tmp/config.toml");
    let cases: &[(&[&str], &[&str])] = &[
        (
            &["--config", "/tmp/cli.toml", "headless"],
            &["before the 'headless' subcommand", "arf headless --config"],
        ),
        (
            &["--config", "/tmp/cli.toml", "r", "resolve"],
            &[
                "before the 'r resolve' subcommand",
                "arf r resolve --config",
            ],
        ),
        (
            &["--config", "/tmp/cli.toml", "config", "check"],
            &[
                "before the 'config check' subcommand",
                "arf config check --config",
            ],
        ),
        (
            &["--config", "/tmp/cli.toml", "completions", "zsh"],
            &["not used by the 'completions' subcommand", "arf --config"],
        ),
    ];

    for (args, expected) in cases {
        assert_top_level_scope_error_with_env(args, expected, Some(config_path));
    }
}

#[test]
fn test_config_check_preserves_environment_path_errors() {
    let mut malformed_config = NamedTempFile::new().expect("Failed to create malformed config");
    write!(malformed_config, "not = [valid").unwrap();

    let malformed_output = sanitized_arf_command()
        .env("ARF_CONFIG", malformed_config.path())
        .args(["config", "check"])
        .output()
        .expect("Failed to check malformed ARF_CONFIG");
    assert_eq!(malformed_output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&malformed_output.stdout).contains(
            malformed_config
                .path()
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
        )
    );
    assert!(String::from_utf8_lossy(&malformed_output.stderr).contains("Config file has errors"));

    let missing_path = malformed_config.path().with_extension("missing.toml");
    let missing_output = sanitized_arf_command()
        .env("ARF_CONFIG", &missing_path)
        .args(["config", "check"])
        .output()
        .expect("Failed to check missing ARF_CONFIG");
    assert_eq!(missing_output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&missing_output.stderr).contains("Config file not found"));
    assert!(
        String::from_utf8_lossy(&missing_output.stderr)
            .contains(missing_path.file_name().unwrap().to_str().unwrap())
    );
}

#[cfg(unix)]
#[test]
fn test_config_check_accepts_non_utf8_arf_config_path() {
    use std::os::unix::ffi::OsStringExt;

    let directory = tempfile::tempdir().expect("Failed to create config directory");
    let file_name = std::ffi::OsString::from_vec(b"config-\xff.toml".to_vec());
    let path = directory.path().join(file_name);
    std::fs::write(&path, "").expect("Failed to write non-UTF-8 config path");

    let output = sanitized_arf_command()
        .env("ARF_CONFIG", &path)
        .args(["config", "check"])
        .output()
        .expect("Failed to run config check with a non-UTF-8 path");

    assert!(
        output.status.success(),
        "config check should preserve non-UTF-8 paths: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Config file is valid."));
}

#[test]
#[cfg(unix)]
fn test_config_check_uses_existing_default_path_without_arf_config() {
    let config_home = tempfile::tempdir().expect("Failed to create temporary config home");
    let expected_config = isolated_default_config_path(config_home.path());
    std::fs::create_dir_all(expected_config.parent().unwrap()).unwrap();
    std::fs::write(&expected_config, "").unwrap();

    let output = sanitized_arf_command()
        .env("HOME", config_home.path())
        .env("USERPROFILE", config_home.path())
        .env("APPDATA", config_home.path())
        .env("XDG_CONFIG_HOME", config_home.path())
        .args(["config", "check"])
        .output()
        .expect("Failed to run arf config check with the default path");

    assert!(
        output.status.success(),
        "config check with default path failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("arf.toml"));
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
fn test_completions_ignores_arf_config_and_still_rejects_cli_config() {
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

    assert_top_level_scope_error(
        &["--config", "/tmp/config.toml", "completions", "zsh"],
        &[
            "--config",
            "not used by the 'completions' subcommand",
            "arf --config",
        ],
    );
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

#[test]
fn test_top_level_no_banner_before_nested_ipc_rejected() {
    assert_top_level_scope_error(
        &["--no-banner", "ipc", "list"],
        &[
            "--no-banner",
            "Usage: arf ipc list",
            "not used by the 'ipc list' subcommand",
            "arf --no-banner",
        ],
    );
}

#[test]
fn test_vanilla_before_headless_rejected() {
    assert_top_level_scope_error(
        &["--vanilla", "headless"],
        &[
            "--vanilla",
            "before the 'headless' subcommand",
            "arf headless --vanilla",
        ],
    );
}

#[test]
fn test_quiet_before_headless_rejected() {
    assert_top_level_scope_error(
        &["--quiet", "headless"],
        &[
            "--quiet",
            "before the 'headless' subcommand",
            "arf headless --quiet",
        ],
    );
}

#[test]
fn test_ipc_bind_before_headless_rejected() {
    assert_top_level_scope_error(
        &["--ipc-bind", "/tmp/x.sock", "headless"],
        &[
            "--ipc-bind",
            "before the 'headless' subcommand",
            "arf headless --ipc-bind <BIND>",
        ],
    );
}

#[test]
fn test_top_level_config_is_consumed_by_history_export() {
    use reedline::SqliteBackedHistory;
    use tempfile::TempDir;

    let history_dir = TempDir::new().expect("Failed to create history directory");
    let r_db = history_dir.path().join("r.db");
    drop(
        SqliteBackedHistory::with_file(r_db, None, None)
            .expect("Failed to create history database"),
    );
    let export_file = history_dir.path().join("export.db");

    let output = sanitized_arf_command()
        .env("ARF_HISTORY_DIR", history_dir.path())
        .args([
            "--config",
            "x",
            "history",
            "export",
            "--file",
            export_file.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run arf history export");

    assert!(
        output.status.success(),
        "top-level --config should be accepted by history: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_top_level_history_dir_is_consumed_by_history_import() {
    use reedline::SqliteBackedHistory;
    use tempfile::TempDir;

    let temp = TempDir::new().expect("Failed to create temporary directory");
    let source_file = temp.path().join("r.db");
    drop(
        SqliteBackedHistory::with_file(source_file.clone(), None, None)
            .expect("Failed to create source history database"),
    );
    let target_dir = temp.path().join("target");

    let output = sanitized_arf_command()
        .args([
            "--history-dir",
            target_dir.to_str().unwrap(),
            "history",
            "import",
            "--from",
            "arf",
            "--file",
            source_file.to_str().unwrap(),
            "--dry-run",
        ])
        .output()
        .expect("Failed to run arf history import");

    assert!(
        output.status.success(),
        "top-level --history-dir should be accepted by history: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_vanilla_without_subcommand_still_works() {
    let output = sanitized_arf_command()
        .args(["--vanilla", "-e", "1 + 1"])
        .output()
        .expect("Failed to run arf");

    assert!(
        output.status.success(),
        "top-level --vanilla without a subcommand should work: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
