use super::*;

/// One top-level option that must be rejected because the subcommand does not
/// consume it, with the stderr fragments that identify the corrected form.
struct ScopeRejection {
    case: &'static str,
    env: &'static [(&'static str, &'static str)],
    args: &'static [&'static str],
    expected: &'static [&'static str],
    forbidden: &'static [&'static str],
}

#[test]
fn test_top_level_options_are_rejected_before_subcommands_that_ignore_them() {
    let cases = [
        ScopeRejection {
            case: "--r-home before headless",
            env: &[],
            args: &["--r-home", "/tmp", "headless"],
            expected: &[
                "--r-home",
                "before the 'headless' subcommand",
                "arf headless --r-home <R_HOME>",
            ],
            forbidden: &[],
        },
        ScopeRejection {
            case: "--no-banner before completions",
            env: &[],
            args: &["--no-banner", "completions", "bash"],
            expected: &[
                "--no-banner",
                "not used by the 'completions' subcommand",
                "arf --no-banner",
            ],
            forbidden: &[],
        },
        ScopeRejection {
            case: "--no-history before history schema",
            env: &[],
            args: &["--no-history", "history", "schema"],
            expected: &[
                "--no-history",
                "not used by the 'history schema' subcommand",
                "arf --no-history",
            ],
            forbidden: &[],
        },
        ScopeRejection {
            case: "--config before config check",
            env: &[],
            args: &["--config", "x", "config", "check"],
            expected: &[
                "Usage: arf config check",
                "before the 'config check' subcommand",
                "arf config check --config <CONFIG>",
            ],
            forbidden: &["arf --config"],
        },
        ScopeRejection {
            case: "--config before headless with ARF_CONFIG set",
            env: &[("ARF_CONFIG", "/tmp/env-config.toml")],
            args: &["--config", "/tmp/cli-config.toml", "headless"],
            expected: &["before the 'headless' subcommand", "arf headless --config"],
            forbidden: &[],
        },
        ScopeRejection {
            case: "--no-banner before nested ipc list",
            env: &[],
            args: &["--no-banner", "ipc", "list"],
            expected: &[
                "--no-banner",
                "Usage: arf ipc list",
                "not used by the 'ipc list' subcommand",
                "arf --no-banner",
            ],
            forbidden: &[],
        },
        ScopeRejection {
            case: "--vanilla before headless",
            env: &[],
            args: &["--vanilla", "headless"],
            expected: &[
                "--vanilla",
                "before the 'headless' subcommand",
                "arf headless --vanilla",
            ],
            forbidden: &[],
        },
        ScopeRejection {
            case: "--quiet before headless",
            env: &[],
            args: &["--quiet", "headless"],
            expected: &[
                "--quiet",
                "before the 'headless' subcommand",
                "arf headless --quiet",
            ],
            forbidden: &[],
        },
        ScopeRejection {
            case: "--ipc-bind before headless",
            env: &[],
            args: &["--ipc-bind", "/tmp/x.sock", "headless"],
            expected: &[
                "--ipc-bind",
                "before the 'headless' subcommand",
                "arf headless --ipc-bind <BIND>",
            ],
            forbidden: &[],
        },
    ];

    for rejection in cases {
        let mut command = sanitized_arf_command();
        command.envs(rejection.env.iter().copied());
        let output = command
            .args(rejection.args)
            .output()
            .unwrap_or_else(|e| panic!("{}: failed to run arf: {e}", rejection.case));
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(
            output.status.code(),
            Some(2),
            "{}: scope errors should use clap's exit code: {stderr}",
            rejection.case
        );
        for text in rejection.expected {
            assert!(
                stderr.contains(text),
                "{}: scope error should contain {text:?}: {stderr}",
                rejection.case
            );
        }
        for text in rejection.forbidden {
            assert!(
                !stderr.contains(text),
                "{}: scope error should not contain {text:?}: {stderr}",
                rejection.case
            );
        }
    }
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
fn test_history_migrate_is_a_noop_without_legacy_history() {
    use tempfile::TempDir;

    let data_home = TempDir::new().expect("Failed to create data directory");
    let state_home = TempDir::new().expect("Failed to create state directory");
    let explicit = TempDir::new().expect("Failed to create explicit history directory");
    let explicit_dir = explicit.path().join("not-created");

    let output = sanitized_arf_command()
        .env("XDG_DATA_HOME", data_home.path())
        .env("XDG_STATE_HOME", state_home.path())
        .args([
            "--history-dir",
            explicit_dir.to_str().unwrap(),
            "history",
            "migrate",
        ])
        .output()
        .expect("Failed to run arf history migrate");

    assert!(
        output.status.success(),
        "history migrate should accept --history-dir: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Linux has distinct legacy and current defaults; elsewhere they coincide.
    #[cfg(target_os = "linux")]
    let expected =
        "Nothing to migrate: no history databases (r.db, shell.db) found in the legacy location";
    #[cfg(not(target_os = "linux"))]
    let expected = "Nothing to migrate: legacy and current default locations are the same";
    assert!(stdout.contains(expected), "unexpected output: {stdout}");
    assert!(
        stdout.contains("only migrates between the default locations"),
        "explicit directory note missing: {stdout}"
    );
    assert!(
        !explicit_dir.exists(),
        "explicit directory must not be created"
    );
    #[cfg(target_os = "linux")]
    for home in [&data_home, &state_home] {
        assert_eq!(
            std::fs::read_dir(home.path()).unwrap().count(),
            0,
            "no-op migration must not create files in the default directories"
        );
    }
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
