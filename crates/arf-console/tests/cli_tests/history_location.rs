//! End-to-end checks of the default history location, driven through the real
//! `HOME` / `XDG_*` environment lookup. Linux only: the XDG variables can
//! redirect the default locations there, whereas Windows resolves them through
//! the Known Folder API, which environment variables cannot redirect.

use super::common::{read_history_rows, sanitized_arf_command};
use std::path::Path;
use std::process::{Command, Output};
use tempfile::TempDir;

fn arf_with_xdg(home: &Path, data_home: &Path, state_home: &Path) -> Command {
    let mut command = sanitized_arf_command();
    command
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env("XDG_DATA_HOME", data_home)
        .env("XDG_STATE_HOME", state_home);
    command
}

fn assert_success(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_legacy_history_is_used_until_migrated_to_the_state_directory() {
    let home = TempDir::new().unwrap();
    let data_home = TempDir::new().unwrap();
    let state_home = TempDir::new().unwrap();
    let legacy_dir = data_home.path().join("arf").join("history");
    let current_dir = state_home.path().join("arf").join("history");

    // Seed a valid legacy history the way a previous arf version would have.
    let rhistory = home.path().join(".Rhistory");
    std::fs::write(&rhistory, "x <- 1\nprint(x)\n").unwrap();
    let output = arf_with_xdg(home.path(), data_home.path(), state_home.path())
        .args(["--history-dir"])
        .arg(&legacy_dir)
        .args(["history", "import", "--from", "r", "--file"])
        .arg(&rhistory)
        .output()
        .unwrap();
    assert_success(&output, "seeding legacy history");
    let legacy_r_db = legacy_dir.join("r.db");
    let legacy_bytes = std::fs::read(&legacy_r_db).unwrap();

    let schema = |what: &str| {
        let output = arf_with_xdg(home.path(), data_home.path(), state_home.path())
            .args(["history", "schema"])
            .output()
            .unwrap();
        assert_success(&output, what);
        String::from_utf8_lossy(&output.stdout).into_owned()
    };

    let before = schema("schema before migrate");
    assert!(
        before.contains(&legacy_r_db.display().to_string()),
        "legacy directory should be in use: {before}"
    );
    assert!(!before.contains(&current_dir.display().to_string()));

    let output = arf_with_xdg(home.path(), data_home.path(), state_home.path())
        .args(["history", "migrate"])
        .output()
        .unwrap();
    assert_success(&output, "history migrate");

    let after = schema("schema after migrate");
    assert!(
        after.contains(&current_dir.join("r.db").display().to_string()),
        "state directory should be in use after migrate: {after}"
    );
    assert!(!after.contains(&legacy_r_db.display().to_string()));

    assert_eq!(
        std::fs::read(&legacy_r_db).unwrap(),
        legacy_bytes,
        "migrate must leave the legacy database unchanged"
    );
    let commands: Vec<String> = read_history_rows(&current_dir.join("r.db"))
        .unwrap()
        .into_iter()
        .map(|row| row.command)
        .collect();
    assert_eq!(commands, ["x <- 1", "print(x)"]);
}
