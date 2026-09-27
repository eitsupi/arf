use super::*;

#[test]
fn test_print_schema_runs() {
    // Resolve the default through the same location resolver used by consumers.
    let _guard = crate::test_utils::lock_env();
    let location =
        crate::config::resolved_history_location(&crate::config::HistoryMode::Persistent {
            dir: None,
        });
    let _ = print_schema(&location);
}

#[test]
fn volatile_schema_does_not_resolve_or_display_a_persistent_path() {
    let location = crate::config::resolved_history_location(&crate::config::HistoryMode::Volatile);
    assert!(location.directory().is_none());
    assert!(print_schema(&location).is_err());
}

#[test]
fn test_generate_schema_lines_contains_expected_content() {
    let history_path = Path::new("/test/path");
    let r_path = history_path.join("r.db");
    let shell_path = history_path.join("shell.db");
    let lines = generate_schema_lines(history_path);

    // Check for expected sections
    assert!(lines.iter().any(|l| l == "# History Database"));
    assert!(lines.iter().any(|l| l == "## Location"));
    assert!(lines.iter().any(|l| l == "## SQLite Schema"));
    assert!(lines.iter().any(|l| l == "## Indexes"));
    assert!(lines.iter().any(|l| l == "## arf Artifact Metadata"));
    assert!(lines.iter().any(|l| l == "## Analyze or Export"));

    // Check for code fences
    assert!(lines.iter().any(|l| l == "```sql"));
    assert!(lines.iter().any(|l| l == "```r"));
    assert!(lines.iter().filter(|l| *l == "```").count() == 3);

    // Check path is included
    assert!(
        lines
            .iter()
            .any(|line| line.contains(&r_path.display().to_string()))
    );
    assert!(
        lines
            .iter()
            .any(|line| line.contains(&shell_path.display().to_string()))
    );

    // Check SQL schema elements
    assert!(lines.iter().any(|l| l.contains("CREATE TABLE history")));
    assert!(lines.iter().any(|l| l.contains("command_line")));
    assert!(lines.iter().any(|l| l.contains("start_timestamp")));
    assert!(
        lines
            .iter()
            .any(|l| l.contains("CREATE TABLE arf_metadata"))
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("key   TEXT PRIMARY KEY NOT NULL"))
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("`history_kind`: `r` or `shell`"))
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("artifact = history-export"))
    );

    // Check R code elements
    assert!(lines.iter().any(|l| l.contains("library(DBI)")));
    assert!(lines.iter().any(|l| l.contains("dbConnect")));
    assert!(lines.iter().any(|l| l.contains("as_tibble")));
}

#[test]
fn test_generate_schema_lines_count() {
    let lines = generate_schema_lines(Path::new("/test/path"));
    // Ensure the schema stays within its expected 50–70 line range.
    assert!(
        lines.len() >= 50,
        "Expected at least 50 lines, got {}",
        lines.len()
    );
    assert!(
        lines.len() <= 70,
        "Expected at most 70 lines, got {}",
        lines.len()
    );
}

#[test]
fn test_style_state_tracks_code_blocks() {
    let mut state = StyleState::new();

    assert_eq!(state.code_block, CodeBlockType::None);

    state.update("```sql");
    assert_eq!(state.code_block, CodeBlockType::Sql);

    state.update("CREATE TABLE test");
    assert_eq!(state.code_block, CodeBlockType::Sql); // Still in SQL block

    state.update("```");
    assert_eq!(state.code_block, CodeBlockType::None);

    state.update("```r");
    assert_eq!(state.code_block, CodeBlockType::R);

    state.update("library(DBI)");
    assert_eq!(state.code_block, CodeBlockType::R); // Still in R block

    state.update("```");
    assert_eq!(state.code_block, CodeBlockType::None);
}

#[test]
fn test_style_sql_line_ratatui_handles_keywords() {
    let line = "    id              INTEGER PRIMARY KEY AUTOINCREMENT,";
    let styled = style_sql_line_ratatui(line);
    // Should have multiple spans (indent, column name, type, comma)
    assert!(styled.spans.len() > 1, "SQL line should have styled spans");
    // Column name "id" should be yellow
    let has_yellow = styled
        .spans
        .iter()
        .any(|s| s.style.fg == Some(RatColor::Yellow));
    assert!(has_yellow, "Column name should be yellow");
}

#[test]
fn test_style_sql_line_ratatui_handles_comments() {
    let line = "    start_timestamp INTEGER,  -- Unix timestamp (nullable)";
    let styled = style_sql_line_ratatui(line);
    // Should contain a comment span with italic style
    let has_italic = styled
        .spans
        .iter()
        .any(|s| s.style.add_modifier.contains(Modifier::ITALIC));
    assert!(has_italic, "Comment should be italic");
    // Verify spacing between comma and comment is preserved
    let full_text: String = styled.spans.iter().map(|s| s.content.as_ref()).collect();
    assert!(
        full_text.contains(",  -- Unix"),
        "Spacing before comment should be preserved: {}",
        full_text
    );
}

#[test]
fn test_style_sql_line_ratatui_padding_unstyled() {
    let line = "    id              INTEGER PRIMARY KEY AUTOINCREMENT,";
    let styled = style_sql_line_ratatui(line);
    // Padding between column name and type should be unstyled (raw)
    let padding_span = styled
        .spans
        .iter()
        .find(|s| s.content.as_ref().chars().all(|c| c == ' ') && s.content.len() > 1);
    assert!(
        padding_span.is_some(),
        "Should have a whitespace-only padding span"
    );
    let ps = padding_span.unwrap();
    assert_eq!(
        ps.style,
        RatStyle::default(),
        "Padding span should be unstyled"
    );
}

#[test]
fn test_style_r_line_ratatui_handles_keywords() {
    let line = "if (TRUE) x else y";
    let styled = style_r_line_ratatui(line);
    // Should have multiple styled spans from tree-sitter
    assert!(
        styled.spans.len() > 1,
        "R line with keywords should have multiple spans"
    );
}

#[test]
fn test_style_r_line_ratatui_handles_strings() {
    let line = r#"  "/path/to/db.db""#;
    let styled = style_r_line_ratatui(line);
    assert!(!styled.spans.is_empty(), "Should have spans");
}

#[test]
fn test_style_r_line_ratatui_handles_operators() {
    let line = "con <- dbConnect(";
    let styled = style_r_line_ratatui(line);
    assert!(
        styled.spans.len() > 1,
        "R line with operators should have multiple spans"
    );
}

#[test]
fn test_style_path_line_ratatui() {
    let line = "- R mode: /home/user/.local/share/arf/history/r.db";
    let styled = style_path_line_ratatui(line);
    // Should have label + green-styled path
    assert_eq!(styled.spans.len(), 2);
    assert_eq!(styled.spans[1].style.fg, Some(RatColor::Green));
}

#[test]
fn test_style_index_line_ratatui() {
    let line = "- idx_history_time        ON history(start_timestamp)";
    let styled = style_index_line_ratatui(line);
    // Should have multiple spans
    assert!(
        styled.spans.len() > 1,
        "Index line should have styled spans"
    );
    // Should contain ON keyword styled in blue bold
    let has_blue_bold = styled.spans.iter().any(|s| {
        s.style.fg == Some(RatColor::Blue)
            && s.style.add_modifier.contains(Modifier::BOLD)
            && s.content.as_ref() == "ON"
    });
    assert!(has_blue_bold, "ON keyword should be blue bold");
}

#[test]
fn test_style_line_to_ratatui_headings() {
    let state = StyleState::new();

    let h1 = style_line_to_ratatui("# History Database", &state);
    let h2 = style_line_to_ratatui("## Location", &state);

    assert!(
        h1.spans[0].style.add_modifier.contains(Modifier::BOLD),
        "H1 should be bold"
    );
    assert!(
        h2.spans[0].style.add_modifier.contains(Modifier::BOLD),
        "H2 should be bold"
    );
}

#[test]
fn test_style_line_to_ratatui_code_fence() {
    let state = StyleState::new();

    let fence = style_line_to_ratatui("```sql", &state);
    assert_eq!(
        fence.spans[0].style.fg,
        Some(RatColor::DarkGray),
        "Code fence should be dark gray"
    );
}

#[cfg(unix)]
#[test]
fn test_schema_output_snapshot() {
    // Use a fixed path to ensure consistent output
    let lines = generate_schema_lines(Path::new("/test/history"));
    let output = lines.join("\n");
    insta::with_settings!({snapshot_path => "../snapshots"}, {
        insta::assert_snapshot!("history_schema_output", output);
    });
}

#[test]
fn test_extract_r_code_block() {
    let lines = generate_schema_lines(Path::new("/test/path"));
    let r_code = extract_r_code_block(&lines);

    // Should contain library calls
    assert!(r_code.contains("library(DBI)"));
    assert!(r_code.contains("library(tibble)"));

    // Should contain dbConnect
    assert!(r_code.contains("dbConnect"));

    // Should contain the path
    assert!(r_code.contains("/test/path/r.db"));

    // Should NOT contain the code fence markers
    assert!(!r_code.contains("```"));
}

#[test]
#[cfg(windows)]
fn r_path_literal_normalizes_windows_separators_and_escapes_quotes() {
    let path = Path::new(r#"C:\Program Files\R "test"\history\r.db"#);
    assert_eq!(
        format_r_path(path),
        r#""C:/Program Files/R \"test\"/history/r.db""#
    );
}

#[cfg(unix)]
#[test]
fn r_path_literal_escapes_unix_backslash_and_quotes() {
    let path = Path::new(r#"/tmp/history\archive/"quoted"/r.db"#);
    assert_eq!(
        format_r_path(path),
        r#""/tmp/history\\archive/\"quoted\"/r.db""#
    );
}

#[test]
fn test_extract_r_code_block_empty_lines() {
    let lines = vec![
        "Some text".to_string(),
        "```r".to_string(),
        "line1".to_string(),
        "".to_string(),
        "line2".to_string(),
        "```".to_string(),
        "More text".to_string(),
    ];
    let r_code = extract_r_code_block(&lines);

    assert_eq!(r_code, "line1\n\nline2");
}

#[test]
fn test_extract_r_code_block_no_r_block() {
    let lines = vec![
        "Some text".to_string(),
        "```sql".to_string(),
        "SELECT * FROM table".to_string(),
        "```".to_string(),
    ];
    let r_code = extract_r_code_block(&lines);

    assert!(r_code.is_empty());
}
