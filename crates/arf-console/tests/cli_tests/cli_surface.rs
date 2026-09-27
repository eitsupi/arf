use super::*;

/// Test that arf binary exists and can show version.
#[test]
fn test_version_flag() {
    let output = sanitized_arf_command()
        .arg("--version")
        .output()
        .expect("Failed to run arf");

    assert!(output.status.success(), "arf --version should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("arf") || stdout.contains("0.1.0"),
        "Version output should contain version info: {}",
        stdout
    );
}

/// Test that arf binary can show help.
#[test]
fn test_help_flag() {
    let output = sanitized_arf_command()
        .arg("--help")
        .output()
        .expect("Failed to run arf");

    assert!(output.status.success(), "arf --help should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--reprex") && stdout.contains("--no-banner") && stdout.contains("--eval"),
        "Help should show CLI options: {}",
        stdout
    );
}

/// Test shell completion generation.
#[test]
fn test_completions_subcommand() {
    let output = sanitized_arf_command()
        .args(["completions", "bash"])
        .output()
        .expect("Failed to run arf completions");

    assert!(
        output.status.success(),
        "arf completions bash should succeed"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("complete") || stdout.contains("arf"),
        "Completion output should contain bash completion code: {}",
        stdout
    );
}

/// Test `arf history schema` subcommand displays schema information.
#[test]
fn test_history_schema_subcommand() {
    let output = sanitized_arf_command()
        .args(["history", "schema"])
        .output()
        .expect("Failed to run arf history schema");

    assert!(output.status.success(), "arf history schema should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Check for expected sections
    assert!(
        stdout.contains("# History Database"),
        "Should contain title: {}",
        stdout
    );
    assert!(
        stdout.contains("## Location"),
        "Should contain location section: {}",
        stdout
    );
    assert!(
        stdout.contains("## SQLite Schema"),
        "Should contain schema section: {}",
        stdout
    );
    assert!(
        stdout.contains("## Indexes"),
        "Should contain indexes section: {}",
        stdout
    );
    assert!(
        stdout.contains("## arf Artifact Metadata"),
        "Should contain arf metadata section: {}",
        stdout
    );
    assert!(
        stdout.contains("## Analyze or Export"),
        "Should contain export section: {}",
        stdout
    );

    // Check for schema content
    assert!(
        stdout.contains("CREATE TABLE history"),
        "Should contain CREATE TABLE: {}",
        stdout
    );
    assert!(
        stdout.contains("command_line"),
        "Should contain command_line column: {}",
        stdout
    );
    assert!(stdout.contains("CREATE TABLE arf_metadata"));
    assert!(stdout.contains("key   TEXT PRIMARY KEY NOT NULL"));
    assert!(stdout.contains("artifact = history-export"));

    // Check for R example
    assert!(
        stdout.contains("library(DBI)"),
        "Should contain R DBI example: {}",
        stdout
    );
    assert!(
        stdout.contains("dbConnect"),
        "Should contain dbConnect: {}",
        stdout
    );
}

#[test]
fn test_history_schema_uses_effective_directory_from_environment() {
    let history_dir = tempfile::tempdir().expect("Failed to create history directory");
    let output = sanitized_arf_command()
        .env("ARF_HISTORY_DIR", history_dir.path())
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

/// Test `arf history schema` outputs plain text when piped.
#[test]
fn test_history_schema_piped_no_colors() {
    let output = sanitized_arf_command()
        .args(["history", "schema"])
        .output()
        .expect("Failed to run arf history schema");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);

    // When piped (not a TTY), output should not contain ANSI escape codes
    assert!(
        !stdout.contains("\x1b["),
        "Piped output should not contain ANSI escape codes: {:?}",
        &stdout[..stdout.len().min(200)]
    );
}

/// Test `arf history import --from arf` rejects self-import for r.db (source == target).
#[test]
fn test_history_import_rejects_self_import_r_db() {
    use reedline::SqliteBackedHistory;
    use tempfile::TempDir;

    // Create a temporary history directory
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let history_dir = temp_dir.path();

    // Create an r.db file using reedline's SqliteBackedHistory
    let r_db_path = history_dir.join("r.db");
    let _db = SqliteBackedHistory::with_file(r_db_path.clone(), None, None)
        .expect("Failed to create r.db");
    drop(_db); // Close the database

    // Try to import from r.db into the same directory's r.db
    // Note: --history-dir is a top-level option, must come before subcommand
    let output = sanitized_arf_command()
        .args([
            "--history-dir",
            history_dir.to_str().unwrap(),
            "history",
            "import",
            "--from",
            "arf",
            "--file",
            r_db_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run arf history import");

    // Should fail with self-import error
    assert!(
        !output.status.success(),
        "Self-import should fail, but succeeded"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Refusing to import") && stderr.contains("into itself"),
        "Error should mention refusing self-import, got: {}",
        stderr
    );
}

/// Test `arf history import --from arf` rejects self-import for shell.db (source == target).
#[test]
fn test_history_import_rejects_self_import_shell_db() {
    use reedline::SqliteBackedHistory;
    use tempfile::TempDir;

    // Create a temporary history directory
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let history_dir = temp_dir.path();

    // Create a shell.db file using reedline's SqliteBackedHistory
    let shell_db_path = history_dir.join("shell.db");
    let _db = SqliteBackedHistory::with_file(shell_db_path.clone(), None, None)
        .expect("Failed to create shell.db");
    drop(_db); // Close the database

    // Try to import from shell.db into the same directory's shell.db
    let output = sanitized_arf_command()
        .args([
            "--history-dir",
            history_dir.to_str().unwrap(),
            "history",
            "import",
            "--from",
            "arf",
            "--file",
            shell_db_path.to_str().unwrap(),
        ])
        .output()
        .expect("Failed to run arf history import");

    // Should fail with self-import error
    assert!(
        !output.status.success(),
        "Self-import of shell.db should fail, but succeeded"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Refusing to import") && stderr.contains("into itself"),
        "Error should mention refusing self-import for shell.db, got: {}",
        stderr
    );
}

/// Test `arf history import --from arf` requires --file option.
#[test]
fn test_history_import_arf_requires_file() {
    let output = sanitized_arf_command()
        .args(["history", "import", "--from", "arf"])
        .output()
        .expect("Failed to run arf history import");

    // Should fail because --file is required for arf format
    assert!(
        !output.status.success(),
        "Import from arf without --file should fail"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--file") && stderr.contains("required"),
        "Error should mention --file is required, got: {}",
        stderr
    );
}
