use super::*;
use rusqlite::Connection;

#[test]
fn test_history_filter_parse_empty() {
    let filter = HistoryFilter::parse("");
    assert!(filter.hostname.is_none());
    assert!(filter.cwd_prefix.is_none());
    assert!(filter.exit_status.is_none());
    assert!(filter.command_pattern.is_empty());
}

#[test]
fn test_history_filter_parse_command_only() {
    let filter = HistoryFilter::parse("git push");
    assert!(filter.hostname.is_none());
    assert!(filter.cwd_prefix.is_none());
    assert!(filter.exit_status.is_none());
    assert_eq!(filter.command_pattern, "git push");
}

#[test]
fn test_history_filter_parse_with_hostname() {
    let filter = HistoryFilter::parse("host:myserver git push");
    assert_eq!(filter.hostname, Some("myserver".to_string()));
    assert_eq!(filter.command_pattern, "git push");
}

#[test]
fn test_history_filter_parse_with_cwd() {
    let filter = HistoryFilter::parse("cwd:/home/user git");
    assert_eq!(filter.cwd_prefix, Some("/home/user".to_string()));
    assert_eq!(filter.command_pattern, "git");
}

#[test]
fn test_history_filter_parse_with_exit_status() {
    let filter = HistoryFilter::parse("exit:0 make");
    assert_eq!(filter.exit_status, Some(0));
    assert_eq!(filter.command_pattern, "make");
}

#[test]
fn test_history_filter_parse_multiple_filters() {
    let filter = HistoryFilter::parse("host:server cwd:/project exit:1 test");
    assert_eq!(filter.hostname, Some("server".to_string()));
    assert_eq!(filter.cwd_prefix, Some("/project".to_string()));
    assert_eq!(filter.exit_status, Some(1));
    assert_eq!(filter.command_pattern, "test");
}

#[test]
fn test_history_filter_parse_invalid_exit_status() {
    let filter = HistoryFilter::parse("exit:abc git");
    assert!(filter.exit_status.is_none());
    // Invalid exit:abc becomes part of command pattern
    assert_eq!(filter.command_pattern, "exit:abc git");
}

#[test]
fn test_calculate_layout_standard_terminal() {
    let (cmd, cwd, host) = calculate_layout(120);
    assert!(cmd >= 20);
    assert!(cwd >= 8);
    assert!(host >= 5);
    // prefix(29) + cmd + space(1) + cwd + space(1) + host = total
    assert_eq!(29 + cmd + 1 + cwd + 1 + host, 120);
}

#[test]
fn test_calculate_layout_80_columns() {
    let (cmd, cwd, host) = calculate_layout(80);
    assert!(cmd >= 20);
    assert!(cwd >= 8);
    assert!(host >= 5);
    assert_eq!(29 + cmd + 1 + cwd + 1 + host, 80);
}

#[test]
fn test_calculate_layout_wide_terminal() {
    let (cmd, cwd, host) = calculate_layout(200);
    assert!(cmd >= 20);
    assert!(cwd <= 20, "cwd_width should be capped at 20, got {}", cwd);
    assert!(
        host <= 15,
        "host_width should be capped at 15, got {}",
        host
    );
    assert_eq!(29 + cmd + 1 + cwd + 1 + host, 200);
}

#[test]
fn test_calculate_layout_narrow_terminal() {
    // Very narrow terminal: cmd_width floors at 20 so total exceeds cols
    let (cmd, cwd, host) = calculate_layout(50);
    assert_eq!(cmd, 20, "cmd_width should floor at 20");
    assert!(cwd >= 8);
    assert!(host >= 5);
    // Total overflows because cmd_width has a minimum of 20
    let total = 29 + cmd + 1 + cwd + 1 + host;
    assert!(
        total > 50,
        "narrow terminal should overflow, total={}",
        total
    );
}

#[test]
fn test_truncate_to_width() {
    assert_eq!(truncate_to_width("hello", 10), "hello");
    assert_eq!(truncate_to_width("hello world", 8), "hello w…");
    assert_eq!(truncate_to_width("hi", 1), "…");
}

#[test]
fn test_exceeds_width() {
    assert!(!exceeds_width("hello", 10));
    assert!(exceeds_width("hello world", 8));
}

#[test]
fn test_scroll_display() {
    // Text that fits
    let (result, max) = scroll_display("hello", 10, 0);
    assert_eq!(result, "hello");
    assert_eq!(max, 0);

    // Text at start
    let (result, _) = scroll_display("hello world", 8, 0);
    assert_eq!(result, "hello w…");

    // Text at end
    let (result, _) = scroll_display("hello world", 8, 100);
    assert_eq!(result, "…o world");
}

#[test]
fn test_db_mode_display_name() {
    assert_eq!(HistoryDbMode::R.display_name(), "R");
    assert_eq!(HistoryDbMode::Shell.display_name(), "Shell");
}

/// Create a temporary history database with test entries.
/// Returns the temp dir (must be kept alive) and the arf-owned store.
///
/// NOTE: The schema here must match reedline's `SqliteBackedHistory` table
/// definition. If reedline changes its schema, this helper must be updated.
fn create_test_db(entries: &[(&str, Option<&str>)]) -> (tempfile::TempDir, HistoryStore) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("test_history.db");
    let db = Connection::open(&db_path).unwrap();
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                command_line TEXT NOT NULL,
                start_timestamp INTEGER,
                session_id INTEGER,
                hostname TEXT,
                cwd TEXT,
                duration_ms INTEGER,
                exit_status INTEGER,
                more_info TEXT
            ) STRICT;",
    )
    .unwrap();
    for (cmd, hostname) in entries {
        db.execute(
            "INSERT INTO history (command_line, hostname) VALUES (?, ?)",
            rusqlite::params![cmd, hostname],
        )
        .unwrap();
    }
    let store = HistoryStore::open(db_path, None, None).unwrap();
    (dir, store)
}

#[test]
fn test_load_history_returns_entries_in_desc_order() {
    let (_dir, store) = create_test_db(&[
        ("first_cmd", Some("host1")),
        ("second_cmd", Some("host2")),
        ("third_cmd", None),
    ]);

    let items = load_history(&store).unwrap();
    assert_eq!(items.len(), 3);
    // Descending order by id
    assert_eq!(items[0].command_line, "third_cmd");
    assert_eq!(items[1].command_line, "second_cmd");
    assert_eq!(items[2].command_line, "first_cmd");
    // Hostname preserved
    assert_eq!(items[1].hostname.as_deref(), Some("host2"));
    assert!(items[0].hostname.is_none());
}

#[test]
fn test_load_history_empty_db() {
    let (_dir, store) = create_test_db(&[]);
    let items = load_history(&store).unwrap();
    assert!(items.is_empty());
}

#[test]
fn test_delete_selected_removes_from_db_and_entries() {
    let (_dir, store) = create_test_db(&[("cmd_a", None), ("cmd_b", None), ("cmd_c", None)]);

    let entries = load_history(&store).unwrap();
    let mut browser = HistoryBrowser::new(entries, HistoryDbMode::R, store.clone());

    // Select the first item (cmd_c, id=3) and the third item (cmd_a, id=1)
    browser.cursor = 0;
    browser.toggle_selection();
    browser.cursor = 2;
    browser.toggle_selection();
    assert_eq!(browser.selected_count(), 2);

    browser.delete_selected().unwrap();

    // Only cmd_b should remain in the browser
    assert_eq!(browser.entries.len(), 1);
    assert_eq!(browser.entries[0].item.command_line, "cmd_b");
    assert_eq!(browser.selected_count(), 0);

    // Verify database state
    let remaining = load_history(&store).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].command_line, "cmd_b");
}

#[test]
fn test_delete_selected_no_selection_is_noop() {
    let (_dir, store) = create_test_db(&[("cmd_a", None)]);
    let entries = load_history(&store).unwrap();
    let mut browser = HistoryBrowser::new(entries, HistoryDbMode::R, store.clone());

    browser.delete_selected().unwrap();

    assert_eq!(browser.entries.len(), 1);
    let remaining = load_history(&store).unwrap();
    assert_eq!(remaining.len(), 1);
}

#[test]
fn test_delete_selected_all_entries() {
    let (_dir, store) = create_test_db(&[("cmd_a", None), ("cmd_b", None)]);
    let entries = load_history(&store).unwrap();
    let mut browser = HistoryBrowser::new(entries, HistoryDbMode::R, store.clone());

    browser.select_all_visible();
    assert_eq!(browser.selected_count(), 2);

    browser.delete_selected().unwrap();

    assert!(browser.entries.is_empty());
    assert!(browser.filtered.is_empty());
    let remaining = load_history(&store).unwrap();
    assert!(remaining.is_empty());
}

#[test]
fn test_flatten_multiline() {
    // Single line - unchanged
    assert_eq!(flatten_multiline("hello"), "hello");
    assert_eq!(flatten_multiline("git push"), "git push");

    // Multiline - newlines replaced with marker
    assert_eq!(flatten_multiline("line1\nline2"), "line1↵line2");
    assert_eq!(
        flatten_multiline("function() {\n  print(1)\n}"),
        "function() {↵  print(1)↵}"
    );

    // Empty string
    assert_eq!(flatten_multiline(""), "");
}

#[test]
fn test_cwd_basename_extraction() {
    use std::path::Path;

    // Helper matching the logic in render()
    fn basename(cwd: &str) -> String {
        Path::new(cwd)
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or(cwd)
            .to_string()
    }

    assert_eq!(basename("/home/user/project"), "project");
    assert_eq!(
        basename("/home/user/my-long-directory-name"),
        "my-long-directory-name"
    );
    assert_eq!(basename("/"), "/");
    assert_eq!(basename(""), "");
    assert_eq!(basename("/foo/bar/baz"), "baz");
    // file_name() returns None for "..", so full path is used as fallback
    assert_eq!(basename("/foo/.."), "/foo/..");
    // file_name() strips trailing "." and returns the parent component
    assert_eq!(basename("/foo/."), "foo");
}
