//! History schema documentation for SQLite-backed command history.
//!
//! This module provides functions to display the history database schema
//! and example R code for accessing it, used by both CLI and REPL commands.

use super::{PagerAction, PagerConfig, PagerContent, copy_to_clipboard, run};
use crate::config::history_dir;
use crate::highlighter::RTreeSitterHighlighter;
use crate::pager::style_convert::styled_text_to_line;
use crossterm::event::{KeyCode, KeyModifiers};
use nu_ansi_term::{Color, Style};
use ratatui::style::{Color as RatColor, Modifier, Style as RatStyle};
use ratatui::text::{Line, Span};
use reedline::Highlighter;
use std::cell::{Cell, RefCell};
use std::io::{self, IsTerminal};

/// Error returned when history directory cannot be determined.
#[derive(Debug)]
pub struct HistoryDirError;

impl std::fmt::Display for HistoryDirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Could not determine history directory")
    }
}

impl std::error::Error for HistoryDirError {}

/// Styles for Markdown-like schema output.
struct SchemaStyles {
    /// Style for headings (# and ##)
    heading: Style,
    /// Style for code fence markers (```)
    code_fence: Style,
    /// Style for file paths
    path: Style,
    /// Style for SQL keywords (CREATE, TABLE, INTEGER, etc.)
    sql_keyword: Style,
    /// Style for SQL identifiers (column names, table name)
    sql_identifier: Style,
    /// Style for SQL comments
    sql_comment: Style,
    /// Style for R keywords (library, function names)
    r_keyword: Style,
    /// Style for R strings
    r_string: Style,
    /// Style for R operators (|>, <-)
    r_operator: Style,
}

impl Default for SchemaStyles {
    fn default() -> Self {
        Self {
            heading: Style::new().bold(),
            code_fence: Style::new().fg(Color::DarkGray),
            path: Style::new().fg(Color::Green),
            sql_keyword: Style::new().fg(Color::Blue).bold(),
            sql_identifier: Style::new().fg(Color::Yellow),
            sql_comment: Style::new().fg(Color::DarkGray).italic(),
            r_keyword: Style::new().fg(Color::Cyan).bold(),
            r_string: Style::new().fg(Color::Green),
            r_operator: Style::new().fg(Color::Magenta),
        }
    }
}

/// Print the history schema documentation to stdout.
///
/// This displays:
/// - Database file locations
/// - SQLite table schema
/// - Example R code for accessing the database
///
/// When stdout is not a terminal (e.g., piped), colors are disabled.
///
/// # Errors
///
/// Returns an error if the history directory cannot be determined.
pub fn print_schema() -> Result<(), HistoryDirError> {
    let history_path = history_dir().ok_or(HistoryDirError)?;
    let history_path = history_path.display().to_string();

    // Check if stdout is a terminal - only use colors if it is
    if io::stdout().is_terminal() {
        print_schema_colored(&history_path);
    } else {
        print_schema_plain(&history_path);
    }

    Ok(())
}

/// Print schema with ANSI colors (for terminal output).
fn print_schema_colored(history_path: &str) {
    let s = SchemaStyles::default();

    // Title
    println!("{}", s.heading.paint("# History Database"));
    println!();

    // Location section
    println!("{}", s.heading.paint("## Location"));
    println!();
    println!(
        "- R mode: {}",
        s.path.paint(format!("{}/r.db", history_path))
    );
    println!(
        "- Shell mode: {}",
        s.path.paint(format!("{}/shell.db", history_path))
    );
    println!();

    // SQLite Schema section
    print_sql_schema(&s);
    println!();

    // Indexes section
    print_indexes(&s);
    println!();

    // R example code
    print_r_example_code(&s, history_path);
}

/// Print schema as plain text (for piped output).
fn print_schema_plain(history_path: &str) {
    // Use the same lines as generate_schema_lines for consistency
    for line in generate_schema_lines(history_path) {
        println!("{}", line);
    }
}

/// Display the history schema in an interactive pager (for REPL use).
///
/// This provides a scrollable view of the schema documentation.
/// Press `q`, `Esc`, or `Ctrl+C/D` to exit. Press `c` to copy R example.
///
/// # Errors
///
/// Returns an error if the history directory cannot be determined.
pub fn show_schema_pager() -> Result<(), HistoryDirError> {
    let history_path = history_dir().ok_or(HistoryDirError)?;
    let history_path = history_path.display().to_string();

    // Generate content lines
    let lines = generate_schema_lines(&history_path);
    let mut content = SchemaContent::new(lines);

    // Configure pager
    let config = PagerConfig {
        title: "History Schema",
        footer_hint: "↑↓/jk scroll │ c copy R example │ q exit",
        manage_alternate_screen: true,
    };

    // Run the pager
    if let Err(e) = run(&mut content, &config) {
        eprintln!("Pager error: {}", e);
    }

    Ok(())
}

/// Generate the schema documentation as a vector of lines.
fn generate_schema_lines(history_path: &str) -> Vec<String> {
    let mut lines = Vec::new();

    // Title
    lines.push("# History Database".to_string());
    lines.push(String::new());

    // Location section
    lines.push("## Location".to_string());
    lines.push(String::new());
    lines.push(format!("- R mode: {}/r.db", history_path));
    lines.push(format!("- Shell mode: {}/shell.db", history_path));
    lines.push(String::new());

    // SQLite Schema section
    lines.push("## SQLite Schema".to_string());
    lines.push(String::new());
    lines.push("```sql".to_string());
    lines.push("CREATE TABLE history (".to_string());
    lines.push("    id              INTEGER PRIMARY KEY AUTOINCREMENT,".to_string());
    lines.push("    command_line    TEXT NOT NULL,".to_string());
    lines.push("    start_timestamp INTEGER,  -- Unix timestamp (nullable)".to_string());
    lines.push("    session_id      INTEGER,".to_string());
    lines.push("    hostname        TEXT,".to_string());
    lines.push("    cwd             TEXT,     -- Current working directory".to_string());
    lines.push("    duration_ms     INTEGER,".to_string());
    lines.push("    exit_status     INTEGER,".to_string());
    lines.push("    more_info       TEXT      -- Reserved for future use".to_string());
    lines.push(");".to_string());
    lines.push("```".to_string());
    lines.push(String::new());

    // Indexes section
    lines.push("## Indexes".to_string());
    lines.push(String::new());
    lines.push("- idx_history_time        ON history(start_timestamp)".to_string());
    lines.push("- idx_history_cwd         ON history(cwd)".to_string());
    lines.push("- idx_history_exit_status ON history(exit_status)".to_string());
    lines.push("- idx_history_cmd         ON history(command_line)".to_string());
    lines.push(String::new());

    // R example code
    lines.push("## Analyze or Export".to_string());
    lines.push(String::new());
    lines.push("Please read the database directly.".to_string());
    lines.push("Example in R:".to_string());
    lines.push(String::new());
    lines.push("```r".to_string());
    lines.push("library(DBI)".to_string());
    lines.push("library(tibble)".to_string());
    lines.push(String::new());
    lines.push("con <- dbConnect(".to_string());
    lines.push("  RSQLite::SQLite(),".to_string());
    lines.push(format!("  \"{}/r.db\"", history_path));
    lines.push(")".to_string());
    lines.push("history_data <- dbGetQuery(".to_string());
    lines.push("  con,".to_string());
    lines.push("  \"SELECT * FROM history ORDER BY id DESC LIMIT 10\"".to_string());
    lines.push(") |>".to_string());
    lines.push("  as_tibble()".to_string());
    lines.push("dbDisconnect(con)".to_string());
    lines.push("```".to_string());

    lines
}

/// Track if we're inside a code block and what type.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum CodeBlockType {
    #[default]
    None,
    Sql,
    R,
}

/// State for tracking code block context during rendering.
#[derive(Clone, Copy, Default)]
struct StyleState {
    code_block: CodeBlockType,
}

impl StyleState {
    fn new() -> Self {
        Self::default()
    }

    fn update(&mut self, line: &str) {
        if line == "```sql" {
            self.code_block = CodeBlockType::Sql;
        } else if line == "```r" {
            self.code_block = CodeBlockType::R;
        } else if line == "```" {
            self.code_block = CodeBlockType::None;
        }
    }
}

/// Content wrapper for displaying schema in the common pager.
struct SchemaContent {
    /// Raw schema lines (unformatted).
    lines: Vec<String>,
    /// Pre-extracted R code for clipboard copy.
    r_code: String,
    /// Current style state for rendering (interior mutability for render_line).
    style_state: Cell<StyleState>,
    /// Feedback message for user actions.
    feedback_message: Option<String>,
}

impl SchemaContent {
    fn new(lines: Vec<String>) -> Self {
        let r_code = extract_r_code_block(&lines);
        Self {
            lines,
            r_code,
            style_state: Cell::new(StyleState::new()),
            feedback_message: None,
        }
    }
}

impl PagerContent for SchemaContent {
    fn line_count(&self) -> usize {
        self.lines.len()
    }

    fn render_line(&self, index: usize, _width: usize) -> Line<'static> {
        let line = &self.lines[index];
        let mut state = self.style_state.get();
        let styled = style_line_to_ratatui(line, &state);
        // Update state for the next line
        state.update(line);
        self.style_state.set(state);
        styled
    }

    fn prepare_render(&mut self, scroll_offset: usize) {
        // Build state by scanning from the beginning up to scroll_offset
        let mut state = StyleState::new();
        for line in self.lines.iter().take(scroll_offset) {
            state.update(line);
        }
        self.style_state.set(state);
    }

    fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Option<PagerAction> {
        // Copy R code block to clipboard
        if code == KeyCode::Char('c') && modifiers == KeyModifiers::NONE {
            if copy_to_clipboard(&self.r_code).is_ok() {
                self.feedback_message = Some("Copied R example to clipboard".to_string());
            } else {
                self.feedback_message = Some("Failed to copy".to_string());
            }
            return None; // Don't exit, just show feedback
        }
        None
    }

    fn feedback_message(&self) -> Option<&str> {
        self.feedback_message.as_deref()
    }

    fn clear_feedback(&mut self) {
        self.feedback_message = None;
    }
}

thread_local! {
    /// Thread-local tree-sitter R highlighter for schema display.
    static R_HIGHLIGHTER: RefCell<RTreeSitterHighlighter> = RefCell::new(RTreeSitterHighlighter::default());
}

// --- ratatui-based style functions (used by PagerContent::render_line) ---

/// Apply syntax highlighting to a line, returning a ratatui `Line`.
fn style_line_to_ratatui(line: &str, state: &StyleState) -> Line<'static> {
    // Headings
    if line.starts_with("# ") || line.starts_with("## ") || line.starts_with("### ") {
        return Line::from(Span::styled(
            line.to_string(),
            RatStyle::default().add_modifier(Modifier::BOLD),
        ));
    }

    // Code fence
    if line.starts_with("```") {
        return Line::from(Span::styled(
            line.to_string(),
            RatStyle::default().fg(RatColor::DarkGray),
        ));
    }

    // Path lines
    if line.starts_with("- R mode:") || line.starts_with("- Shell mode:") {
        return style_path_line_ratatui(line);
    }

    // Index lines
    if line.starts_with("- idx_") {
        return style_index_line_ratatui(line);
    }

    // Style based on code block context
    match state.code_block {
        CodeBlockType::Sql => style_sql_line_ratatui(line),
        CodeBlockType::R => style_r_line_ratatui(line),
        CodeBlockType::None => Line::from(line.to_string()),
    }
}

/// Style a SQL line for ratatui rendering.
fn style_sql_line_ratatui(line: &str) -> Line<'static> {
    let kw = RatStyle::default()
        .fg(RatColor::Blue)
        .add_modifier(Modifier::BOLD);
    let ident = RatStyle::default().fg(RatColor::Yellow);
    let comment_style = RatStyle::default()
        .fg(RatColor::DarkGray)
        .add_modifier(Modifier::ITALIC);

    // CREATE TABLE line
    if line.starts_with("CREATE TABLE") {
        return Line::from(vec![
            Span::styled("CREATE", kw),
            Span::raw(" "),
            Span::styled("TABLE", kw),
            Span::raw(" "),
            Span::styled("history", ident),
            Span::raw(" ("),
        ]);
    }

    // Closing paren
    if line == ");" {
        return Line::from(line.to_string());
    }

    // Column definitions (lines starting with 4 spaces)
    if line.starts_with("    ") {
        let trimmed = line.trim_start();
        let indent = Span::raw("    ");

        // Split off the comment if present
        let (code_part, comment_part) = if let Some(idx) = trimmed.find("--") {
            (&trimmed[..idx], Some(&trimmed[idx..]))
        } else {
            (trimmed, None)
        };

        // Parse "column_name    TYPE [EXTRA]," from code_part
        let code_trimmed = code_part.trim_end();
        let has_comma = code_trimmed.ends_with(',');
        let code_no_comma = code_trimmed.trim_end_matches(',');

        // Split at first run of spaces to separate column name from type
        let mut spans = vec![indent];
        if let Some(space_idx) = code_no_comma.find(' ') {
            let col_name = &code_no_comma[..space_idx];
            let rest = &code_no_comma[space_idx..];
            // Separate leading whitespace (alignment padding) from type keywords
            let type_start = rest.len() - rest.trim_start().len();
            let padding = &rest[..type_start];
            let type_part = &rest[type_start..];
            spans.push(Span::styled(col_name.to_string(), ident));
            spans.push(Span::raw(padding.to_string()));
            spans.push(Span::styled(type_part.to_string(), kw));
        } else {
            spans.push(Span::styled(code_no_comma.to_string(), ident));
        }

        if has_comma {
            spans.push(Span::raw(","));
        }

        if let Some(comment) = comment_part {
            // Restore spacing between code and comment that was stripped by trim_end()
            let gap_len = code_part.len() - code_trimmed.len();
            if gap_len > 0 {
                spans.push(Span::raw(" ".repeat(gap_len)));
            }
            spans.push(Span::styled(comment.to_string(), comment_style));
        }

        return Line::from(spans);
    }

    Line::from(line.to_string())
}

/// Style an R code line for ratatui rendering using tree-sitter.
fn style_r_line_ratatui(line: &str) -> Line<'static> {
    R_HIGHLIGHTER.with(|highlighter| {
        let styled = highlighter.borrow().highlight(line, 0);
        styled_text_to_line(&styled)
    })
}

/// Style a path line for ratatui rendering.
fn style_path_line_ratatui(line: &str) -> Line<'static> {
    if let Some(colon_idx) = line.find(": ") {
        let (label, path) = line.split_at(colon_idx + 2);
        Line::from(vec![
            Span::raw(label.to_string()),
            Span::styled(path.to_string(), RatStyle::default().fg(RatColor::Green)),
        ])
    } else {
        Line::from(line.to_string())
    }
}

/// Style an index line for ratatui rendering.
fn style_index_line_ratatui(line: &str) -> Line<'static> {
    let kw = RatStyle::default()
        .fg(RatColor::Blue)
        .add_modifier(Modifier::BOLD);
    let ident = RatStyle::default().fg(RatColor::Yellow);

    // Parse: "- idx_name        ON history(column)"
    if let Some(on_idx) = line.find(" ON ") {
        let before_on = &line[..on_idx];
        let after_on = &line[on_idx + 4..]; // skip " ON "

        // before_on: "- idx_name      "
        // Split into "- " prefix and index name
        let mut spans = Vec::new();
        if let Some(idx_start) = before_on.find("idx_") {
            spans.push(Span::raw(before_on[..idx_start].to_string()));
            spans.push(Span::styled(
                before_on[idx_start..].trim_end().to_string(),
                ident,
            ));
            // Preserve spacing between index name and ON
            let name_end = before_on[idx_start..]
                .find(' ')
                .map(|i| idx_start + i)
                .unwrap_or(before_on.len());
            let spacing = &before_on[name_end..];
            spans.push(Span::raw(spacing.to_string()));
        } else {
            spans.push(Span::raw(before_on.to_string()));
        }

        spans.push(Span::styled("ON".to_string(), kw));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(after_on.to_string(), ident));

        Line::from(spans)
    } else {
        Line::from(line.to_string())
    }
}

/// Print the SQLite schema in a code block.
fn print_sql_schema(s: &SchemaStyles) {
    println!("{}", s.heading.paint("## SQLite Schema"));
    println!();
    println!("{}", s.code_fence.paint("```sql"));
    println!(
        "{} {} {} (",
        s.sql_keyword.paint("CREATE"),
        s.sql_keyword.paint("TABLE"),
        s.sql_identifier.paint("history")
    );
    println!(
        "    {}              {},",
        s.sql_identifier.paint("id"),
        s.sql_keyword.paint("INTEGER PRIMARY KEY AUTOINCREMENT")
    );
    println!(
        "    {}    {} {},",
        s.sql_identifier.paint("command_line"),
        s.sql_keyword.paint("TEXT"),
        s.sql_keyword.paint("NOT NULL")
    );
    println!(
        "    {} {},  {}",
        s.sql_identifier.paint("start_timestamp"),
        s.sql_keyword.paint("INTEGER"),
        s.sql_comment.paint("-- Unix timestamp (nullable)")
    );
    println!(
        "    {}      {},",
        s.sql_identifier.paint("session_id"),
        s.sql_keyword.paint("INTEGER")
    );
    println!(
        "    {}        {},",
        s.sql_identifier.paint("hostname"),
        s.sql_keyword.paint("TEXT")
    );
    println!(
        "    {}             {},     {}",
        s.sql_identifier.paint("cwd"),
        s.sql_keyword.paint("TEXT"),
        s.sql_comment.paint("-- Current working directory")
    );
    println!(
        "    {}     {},",
        s.sql_identifier.paint("duration_ms"),
        s.sql_keyword.paint("INTEGER")
    );
    println!(
        "    {}     {},",
        s.sql_identifier.paint("exit_status"),
        s.sql_keyword.paint("INTEGER")
    );
    println!(
        "    {}       {}      {}",
        s.sql_identifier.paint("more_info"),
        s.sql_keyword.paint("TEXT"),
        s.sql_comment.paint("-- Reserved for future use")
    );
    println!(");");
    println!("{}", s.code_fence.paint("```"));
}

/// Print the index definitions.
fn print_indexes(s: &SchemaStyles) {
    println!("{}", s.heading.paint("## Indexes"));
    println!();
    println!(
        "- {}        {} {}({})",
        s.sql_identifier.paint("idx_history_time"),
        s.sql_keyword.paint("ON"),
        s.sql_identifier.paint("history"),
        s.sql_identifier.paint("start_timestamp")
    );
    println!(
        "- {}         {} {}({})",
        s.sql_identifier.paint("idx_history_cwd"),
        s.sql_keyword.paint("ON"),
        s.sql_identifier.paint("history"),
        s.sql_identifier.paint("cwd")
    );
    println!(
        "- {} {} {}({})",
        s.sql_identifier.paint("idx_history_exit_status"),
        s.sql_keyword.paint("ON"),
        s.sql_identifier.paint("history"),
        s.sql_identifier.paint("exit_status")
    );
    println!(
        "- {}         {} {}({})",
        s.sql_identifier.paint("idx_history_cmd"),
        s.sql_keyword.paint("ON"),
        s.sql_identifier.paint("history"),
        s.sql_identifier.paint("command_line")
    );
}

/// Print example R code for accessing the history database.
fn print_r_example_code(s: &SchemaStyles, history_path: &str) {
    println!("{}", s.heading.paint("## Analyze or Export"));
    println!();
    println!("Please read the database directly.");
    println!("Example in R:");
    println!();
    println!("{}", s.code_fence.paint("```r"));

    // library(DBI)
    println!(
        "{}({})",
        s.r_keyword.paint("library"),
        s.r_keyword.paint("DBI")
    );
    // library(tibble)
    println!(
        "{}({})",
        s.r_keyword.paint("library"),
        s.r_keyword.paint("tibble")
    );
    println!();

    // con <- dbConnect(...)
    println!(
        "con {} {}(",
        s.r_operator.paint("<-"),
        s.r_keyword.paint("dbConnect")
    );
    println!("  RSQLite::{}(),", s.r_keyword.paint("SQLite"));
    println!(
        "  {}",
        s.r_string.paint(format!("\"{}/r.db\"", history_path))
    );
    println!(")");

    // history_data <- dbGetQuery(...) |> as_tibble()
    println!(
        "history_data {} {}(",
        s.r_operator.paint("<-"),
        s.r_keyword.paint("dbGetQuery")
    );
    println!("  con,");
    println!(
        "  {}",
        s.r_string
            .paint("\"SELECT * FROM history ORDER BY id DESC LIMIT 10\"")
    );
    println!(") {}", s.r_operator.paint("|>"));
    println!("  {}()", s.r_keyword.paint("as_tibble"));

    // dbDisconnect(con)
    println!("{}(con)", s.r_keyword.paint("dbDisconnect"));

    println!("{}", s.code_fence.paint("```"));
}

/// Extract the R code block content from schema lines.
///
/// Returns the lines between ```r and ```, excluding the fence markers.
fn extract_r_code_block(lines: &[String]) -> String {
    let mut in_r_block = false;
    let mut code_lines = Vec::new();

    for line in lines {
        if line == "```r" {
            in_r_block = true;
            continue;
        }
        if line == "```" && in_r_block {
            break;
        }
        if in_r_block {
            code_lines.push(line.as_str());
        }
    }

    code_lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_print_schema_runs() {
        // On systems with XDG support, this should succeed
        // On other systems (or in restrictive environments), it may fail
        let _ = print_schema();
    }

    #[test]
    fn test_generate_schema_lines_contains_expected_content() {
        let lines = generate_schema_lines("/test/path");

        // Check for expected sections
        assert!(lines.iter().any(|l| l == "# History Database"));
        assert!(lines.iter().any(|l| l == "## Location"));
        assert!(lines.iter().any(|l| l == "## SQLite Schema"));
        assert!(lines.iter().any(|l| l == "## Indexes"));
        assert!(lines.iter().any(|l| l == "## Analyze or Export"));

        // Check for code fences
        assert!(lines.iter().any(|l| l == "```sql"));
        assert!(lines.iter().any(|l| l == "```r"));
        assert!(lines.iter().filter(|l| *l == "```").count() == 2);

        // Check path is included
        assert!(lines.iter().any(|l| l.contains("/test/path/r.db")));
        assert!(lines.iter().any(|l| l.contains("/test/path/shell.db")));

        // Check SQL schema elements
        assert!(lines.iter().any(|l| l.contains("CREATE TABLE history")));
        assert!(lines.iter().any(|l| l.contains("command_line")));
        assert!(lines.iter().any(|l| l.contains("start_timestamp")));

        // Check R code elements
        assert!(lines.iter().any(|l| l.contains("library(DBI)")));
        assert!(lines.iter().any(|l| l.contains("dbConnect")));
        assert!(lines.iter().any(|l| l.contains("as_tibble")));
    }

    #[test]
    fn test_generate_schema_lines_count() {
        let lines = generate_schema_lines("/test/path");
        // Ensure we have a reasonable number of lines (schema should be ~50 lines)
        assert!(
            lines.len() >= 40,
            "Expected at least 40 lines, got {}",
            lines.len()
        );
        assert!(
            lines.len() <= 60,
            "Expected at most 60 lines, got {}",
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

    #[test]
    fn test_schema_output_snapshot() {
        // Use a fixed path to ensure consistent output
        let lines = generate_schema_lines("/test/history");
        let output = lines.join("\n");
        insta::assert_snapshot!("history_schema_output", output);
    }

    #[test]
    fn test_extract_r_code_block() {
        let lines = generate_schema_lines("/test/path");
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
}
