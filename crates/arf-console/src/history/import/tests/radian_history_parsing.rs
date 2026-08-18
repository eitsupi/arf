use super::super::*;
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn test_parse_radian_history_basic() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
    writeln!(file, "# mode: r").unwrap();
    writeln!(file, "+library(dplyr)").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "# time: 2024-01-15 10:31:00 UTC").unwrap();
    writeln!(file, "# mode: shell").unwrap();
    writeln!(file, "+ls -la").unwrap();

    let entries = parse_radian_history(file.path()).unwrap();
    assert_eq!(entries.len(), 2);

    assert_eq!(entries[0].command, "library(dplyr)");
    assert_eq!(entries[0].mode, Some("r".to_string()));
    assert!(entries[0].timestamp.is_some());

    assert_eq!(entries[1].command, "ls -la");
    assert_eq!(entries[1].mode, Some("shell".to_string()));
}

#[test]
fn test_parse_radian_history_multiline() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
    writeln!(file, "# mode: r").unwrap();
    writeln!(file, "+iris %>%").unwrap();
    writeln!(file, r#"+  filter(Species == "setosa") %>%"#).unwrap();
    writeln!(file, "+  head()").unwrap();

    let entries = parse_radian_history(file.path()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0].command,
        r#"iris %>%
  filter(Species == "setosa") %>%
  head()"#
    );
}

#[test]
fn test_parse_radian_history_empty_file() {
    let file = NamedTempFile::new().unwrap();
    let entries = parse_radian_history(file.path()).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn test_parse_radian_history_timestamp_parsing() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "# time: 2024-06-15 14:30:45 UTC").unwrap();
    writeln!(file, "# mode: r").unwrap();
    writeln!(file, "+test()").unwrap();

    let entries = parse_radian_history(file.path()).unwrap();
    assert_eq!(entries.len(), 1);

    let ts = entries[0].timestamp.unwrap();
    assert_eq!(
        ts.format("%Y-%m-%d %H:%M:%S").to_string(),
        "2024-06-15 14:30:45"
    );
}

#[test]
fn test_parse_radian_history_browse_mode() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
    writeln!(file, "# mode: browse").unwrap();
    writeln!(file, "+n").unwrap();

    let entries = parse_radian_history(file.path()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].mode, Some("browse".to_string()));
}

// === Edge case tests for regression prevention ===

#[test]
fn test_parse_radian_history_mode_not_carried_over() {
    // Regression test: mode should NOT carry over from previous entry
    // when a new timestamp boundary is encountered without a mode line.
    let mut file = NamedTempFile::new().unwrap();
    // First entry with explicit shell mode
    writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
    writeln!(file, "# mode: shell").unwrap();
    writeln!(file, "+ls -la").unwrap();
    writeln!(file).unwrap();
    // Second entry WITHOUT mode line - should NOT inherit "shell" from previous
    writeln!(file, "# time: 2024-01-15 10:31:00 UTC").unwrap();
    writeln!(file, "+library(dplyr)").unwrap();

    let entries = parse_radian_history(file.path()).unwrap();
    assert_eq!(entries.len(), 2);

    // First entry should be shell
    assert_eq!(entries[0].command, "ls -la");
    assert_eq!(entries[0].mode, Some("shell".to_string()));

    // Second entry should have no mode (None), not "shell"
    assert_eq!(entries[1].command, "library(dplyr)");
    assert_eq!(entries[1].mode, None);
}

#[test]
fn test_parse_radian_history_consecutive_timestamps_without_commands() {
    // Edge case: consecutive timestamp headers without commands should not cause issues
    let mut file = NamedTempFile::new().unwrap();
    // First timestamp with no command lines
    writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
    writeln!(file, "# mode: r").unwrap();
    // Empty line acts as separator
    writeln!(file).unwrap();
    // Second timestamp immediately follows
    writeln!(file, "# time: 2024-01-15 10:31:00 UTC").unwrap();
    writeln!(file, "# mode: shell").unwrap();
    writeln!(file, "+git status").unwrap();

    let entries = parse_radian_history(file.path()).unwrap();
    // Only one entry should be parsed (the one with actual command)
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].command, "git status");
    assert_eq!(entries[0].mode, Some("shell".to_string()));
}

#[test]
fn test_parse_radian_history_file_not_found() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let missing_path = temp_dir.path().join("nonexistent_radian_history");

    let result = parse_radian_history(&missing_path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Failed to open radian history"));
}

#[test]
fn test_parse_radian_history_mode_reset_between_entries() {
    // Another regression test: ensure mode is properly reset between entries
    // even when separated by empty lines
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "# time: 2024-01-15 10:30:00 UTC").unwrap();
    writeln!(file, "# mode: shell").unwrap();
    writeln!(file, "+pwd").unwrap();
    writeln!(file).unwrap(); // Empty line separator
    writeln!(file, "# time: 2024-01-15 10:31:00 UTC").unwrap();
    // No mode line for this entry
    writeln!(file, "+summary(iris)").unwrap();
    writeln!(file).unwrap();
    writeln!(file, "# time: 2024-01-15 10:32:00 UTC").unwrap();
    writeln!(file, "# mode: browse").unwrap();
    writeln!(file, "+n").unwrap();

    let entries = parse_radian_history(file.path()).unwrap();
    assert_eq!(entries.len(), 3);

    assert_eq!(entries[0].mode, Some("shell".to_string()));
    assert_eq!(entries[1].mode, None); // Mode was reset, not carried over
    assert_eq!(entries[2].mode, Some("browse".to_string()));
}

#[test]
fn test_parse_radian_history_crlf_line_endings() {
    // Test that CRLF line endings (Windows) are handled correctly
    let mut file = NamedTempFile::new().unwrap();
    // Write with explicit \r\n
    file.write_all(b"# time: 2024-01-15 10:30:00 UTC\r\n")
        .unwrap();
    file.write_all(b"# mode: r\r\n").unwrap();
    file.write_all(b"+print(1)\r\n").unwrap();

    let entries = parse_radian_history(file.path()).unwrap();
    assert_eq!(entries.len(), 1);
    // Command should not have trailing \r
    assert_eq!(entries[0].command, "print(1)");
    assert!(!entries[0].command.ends_with('\r'));
}

#[test]
fn test_parse_radian_history_colon_command_has_no_metadata() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "# time: 2024-06-15 14:30:45 UTC").unwrap();
    writeln!(file, "# mode: r").unwrap();
    writeln!(file, "+:help(topic)").unwrap();

    let parsed = parse_radian_history(file.path()).unwrap();
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entries[0].metadata, None);
}
