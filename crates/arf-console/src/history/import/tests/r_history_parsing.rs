use super::super::*;
use std::io::Write;
use tempfile::NamedTempFile;

#[test]
fn test_parse_r_history_basic() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "library(dplyr)").unwrap();
    writeln!(file, "print(\"hello\")").unwrap();
    writeln!(file).unwrap(); // Empty line should be skipped
    writeln!(file, "summary(iris)").unwrap();

    let entries = parse_r_history(file.path()).unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0].command, "library(dplyr)");
    assert_eq!(entries[1].command, "print(\"hello\")");
    assert_eq!(entries[2].command, "summary(iris)");

    // All entries should have mode "r" and no timestamp
    for entry in &entries {
        assert_eq!(entry.mode, Some("r".to_string()));
        assert!(entry.timestamp.is_none());
    }
}

#[test]
fn test_parse_r_history_empty_file() {
    let file = NamedTempFile::new().unwrap();
    let entries = parse_r_history(file.path()).unwrap();
    assert!(entries.is_empty());
}

#[test]
fn test_parse_r_history_preserves_leading_whitespace() {
    let mut file = NamedTempFile::new().unwrap();
    // Simulate indented code that might appear in .Rhistory
    writeln!(file, "if (TRUE) {{").unwrap();
    writeln!(file, r#"  print("indented")"#).unwrap();
    writeln!(file, "}}").unwrap();

    let entries = parse_r_history(file.path()).unwrap();
    assert_eq!(entries.len(), 3);
    // Leading whitespace should be preserved
    assert_eq!(entries[1].command, r#"  print("indented")"#);
}

#[test]
fn test_default_paths() {
    // default_radian_path reads HOME through dirs::home_dir, and
    // default_r_history_path reads R_HISTFILE through std::env::var.
    let _guard = crate::test_utils::lock_env();
    // These just verify the functions don't panic
    let radian_path = default_radian_path();
    assert!(radian_path.to_string_lossy().contains("radian_history"));

    let r_path = default_r_history_path();
    assert!(r_path.to_string_lossy().contains("Rhistory") || std::env::var("R_HISTFILE").is_ok());
}

#[test]
fn test_import_entry_struct() {
    let entry = ImportEntry {
        command: "test".to_string(),
        timestamp: Some(Utc::now()),
        mode: Some("r".to_string()),
    };
    assert_eq!(entry.command, "test");
    assert!(entry.timestamp.is_some());
    assert_eq!(entry.mode, Some("r".to_string()));
}

#[test]
fn test_parse_r_history_file_not_found() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let missing_path = temp_dir.path().join("nonexistent_Rhistory");

    let result = parse_r_history(&missing_path);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Failed to open R history"));
}

#[test]
fn test_parse_r_history_whitespace_only_lines_skipped() {
    let mut file = NamedTempFile::new().unwrap();
    writeln!(file, "library(dplyr)").unwrap();
    writeln!(file, "   ").unwrap(); // Whitespace-only line
    writeln!(file, "\t").unwrap(); // Tab-only line
    writeln!(file, "print(1)").unwrap();

    let entries = parse_r_history(file.path()).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].command, "library(dplyr)");
    assert_eq!(entries[1].command, "print(1)");
}
