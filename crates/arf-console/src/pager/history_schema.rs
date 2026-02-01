//! History schema documentation for SQLite-backed command history.
//!
//! This module provides functions to display the history database schema
//! and example R code for accessing it, used by both CLI and REPL commands.

use super::{PagerAction, PagerConfig, PagerContent, run};
use crate::config::history_dir;
use crate::highlighter::RTreeSitterHighlighter;
use base64::{Engine, engine::general_purpose};
use crossterm::{Command, event::KeyCode, event::KeyModifiers};
use nu_ansi_term::{Color, Style};
use reedline::Highlighter;
use std::cell::{Cell, RefCell};
use std::io::{self, BufWriter, IsTerminal};

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

    fn render_line(&self, index: usize, _width: usize) -> String {
        let line = &self.lines[index];
        let mut state = self.style_state.get();
        let styled = style_line_with_state(line, &state);
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

/// Apply syntax highlighting to a line based on its content and context.
fn style_line_with_state(line: &str, state: &StyleState) -> String {
    use crossterm::style::Stylize;

    // Headings
    if line.starts_with("# ") || line.starts_with("## ") || line.starts_with("### ") {
        return line.bold().to_string();
    }

    // Code fence
    if line.starts_with("```") {
        return line.dark_grey().to_string();
    }

    // Path lines
    if line.starts_with("- R mode:") || line.starts_with("- Shell mode:") {
        return style_path_line(line);
    }

    // Index lines
    if line.starts_with("- idx_") {
        return style_index_line(line);
    }

    // Style based on code block context
    match state.code_block {
        CodeBlockType::Sql => style_sql_line(line),
        CodeBlockType::R => style_r_line(line),
        CodeBlockType::None => line.to_string(),
    }
}

/// Style a SQL line with syntax highlighting.
fn style_sql_line(line: &str) -> String {
    use crossterm::style::Stylize;

    // CREATE TABLE line
    if line.starts_with("CREATE TABLE") {
        return line
            .replace("CREATE", &"CREATE".blue().bold().to_string())
            .replace("TABLE", &"TABLE".blue().bold().to_string())
            .replace("history", &"history".yellow().to_string());
    }

    // Column definitions
    if line.starts_with("    ") {
        let mut result = line.to_string();

        // Find and style the comment part first
        if let Some(comment_idx) = result.find("--") {
            let (code_part, comment_part) = result.split_at(comment_idx);
            let styled_comment = comment_part.dark_grey().italic().to_string();
            result = format!("{}{}", code_part, styled_comment);
        }

        // Style SQL keywords (blue bold)
        result = result
            .replace(
                "INTEGER PRIMARY KEY AUTOINCREMENT",
                &"INTEGER PRIMARY KEY AUTOINCREMENT"
                    .blue()
                    .bold()
                    .to_string(),
            )
            .replace("INTEGER", &"INTEGER".blue().bold().to_string())
            .replace("TEXT NOT NULL", &"TEXT NOT NULL".blue().bold().to_string())
            .replace("TEXT", &"TEXT".blue().bold().to_string());

        // Style column names (yellow) - they come at the start after spaces
        let column_names = [
            "id",
            "command_line",
            "start_timestamp",
            "session_id",
            "hostname",
            "cwd",
            "duration_ms",
            "exit_status",
            "more_info",
        ];
        for name in column_names {
            // Match column name at start of trimmed line
            let pattern = format!("    {}", name);
            if result.contains(&pattern) {
                result = result.replace(&pattern, &format!("    {}", name.yellow()));
            }
        }

        return result;
    }

    line.to_string()
}

thread_local! {
    /// Thread-local tree-sitter R highlighter for schema display.
    static R_HIGHLIGHTER: RefCell<RTreeSitterHighlighter> = RefCell::new(RTreeSitterHighlighter::default());
}

/// Style an R code line with syntax highlighting using tree-sitter.
fn style_r_line(line: &str) -> String {
    // Use tree-sitter based highlighting for accurate tokenization
    R_HIGHLIGHTER.with(|highlighter| {
        let styled = highlighter.borrow().highlight(line, 0);

        // Convert StyledText to ANSI string
        let mut result = String::new();
        for (style, text) in &styled.buffer {
            result.push_str(&format!("{}", style.paint(text)));
        }
        result
    })
}

/// Style a path line (Location section).
fn style_path_line(line: &str) -> String {
    use crossterm::style::Stylize;

    if let Some(colon_idx) = line.find(": ") {
        let (label, path) = line.split_at(colon_idx + 2);
        format!("{}{}", label, path.green())
    } else {
        line.to_string()
    }
}

/// Style an index line.
fn style_index_line(line: &str) -> String {
    use crossterm::style::Stylize;

    // Highlight index name (yellow) and ON keyword (blue)
    let mut result = line.to_string();
    result = result.replace(" ON ", &format!(" {} ", "ON".blue().bold()));

    // Highlight index names
    let index_names = [
        "idx_history_time",
        "idx_history_cwd",
        "idx_history_exit_status",
        "idx_history_cmd",
    ];
    for name in index_names {
        result = result.replace(name, &name.yellow().to_string());
    }

    result
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

/// OSC 52 clipboard command for copying text via terminal escape sequence.
///
/// Reference: <https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Operating-System-Commands>
/// Based on television's implementation: refs/television/television/utils/clipboard.rs
struct SetClipboard {
    content: String,
}

impl SetClipboard {
    fn new(content: &str) -> Self {
        Self {
            content: general_purpose::STANDARD.encode(content.as_bytes()),
        }
    }
}

impl Command for SetClipboard {
    fn write_ansi(&self, f: &mut impl std::fmt::Write) -> std::fmt::Result {
        write!(f, "\x1b]52;c;{}\x1b\\", self.content)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        // OSC 52 is ANSI-based, no WinAPI implementation needed.
        // Modern Windows terminals support ANSI sequences.
        Ok(())
    }
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

/// Copy text to clipboard using OSC 52 escape sequence.
fn copy_to_clipboard(text: &str) -> io::Result<()> {
    let mut writer = BufWriter::new(io::stderr());
    crossterm::execute!(writer, SetClipboard::new(text))
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
    fn test_style_sql_line_handles_keywords() {
        let line = "    id              INTEGER PRIMARY KEY AUTOINCREMENT,";
        let styled = style_sql_line(line);

        // Should contain ANSI codes for styling
        assert!(
            styled.len() > line.len(),
            "Styled line should be longer due to ANSI codes"
        );
        // Original text should still be present (somewhere in the styled output)
        assert!(styled.contains("id") || styled.contains("\x1b")); // Either plain or with escape codes
    }

    #[test]
    fn test_style_sql_line_handles_comments() {
        let line = "    start_timestamp INTEGER,  -- Unix timestamp (nullable)";
        let styled = style_sql_line(line);

        // Should contain ANSI codes
        assert!(
            styled.len() > line.len(),
            "Styled line should be longer due to ANSI codes"
        );
    }

    #[test]
    fn test_style_r_line_handles_keywords() {
        // Use actual R keywords (if, function, etc.) which are styled by tree-sitter
        let line = "if (TRUE) x else y";
        let styled = style_r_line(line);

        // Should contain ANSI codes for keywords (if, else) and constants (TRUE)
        assert!(
            styled.len() > line.len(),
            "Styled line should be longer due to ANSI codes"
        );
    }

    #[test]
    fn test_style_r_line_handles_strings() {
        let line = r#"  "/path/to/db.db""#;
        let styled = style_r_line(line);

        // Should contain ANSI codes for green string
        assert!(styled.contains("\x1b"), "Should contain ANSI escape codes");
    }

    #[test]
    fn test_style_r_line_handles_operators() {
        let line = "con <- dbConnect(";
        let styled = style_r_line(line);

        // Should contain ANSI codes
        assert!(
            styled.len() > line.len(),
            "Styled line should be longer due to ANSI codes"
        );
    }

    #[test]
    fn test_style_path_line() {
        let line = "- R mode: /home/user/.local/share/arf/history/r.db";
        let styled = style_path_line(line);

        // Should contain ANSI codes for green path
        assert!(styled.contains("\x1b"), "Should contain ANSI escape codes");
    }

    #[test]
    fn test_style_index_line() {
        let line = "- idx_history_time        ON history(start_timestamp)";
        let styled = style_index_line(line);

        // Should contain ANSI codes
        assert!(styled.contains("\x1b"), "Should contain ANSI escape codes");
    }

    #[test]
    fn test_style_line_with_state_headings() {
        let state = StyleState::new();

        let h1 = style_line_with_state("# History Database", &state);
        let h2 = style_line_with_state("## Location", &state);
        let h2b = style_line_with_state("## Indexes", &state);

        // Headings should be styled (bold)
        assert!(h1.contains("\x1b"), "H1 should be styled");
        assert!(h2.contains("\x1b"), "H2 should be styled");
        assert!(h2b.contains("\x1b"), "H2 should be styled");
    }

    #[test]
    fn test_style_line_with_state_code_fence() {
        let state = StyleState::new();

        let fence = style_line_with_state("```sql", &state);
        assert!(
            fence.contains("\x1b"),
            "Code fence should be styled (dark grey)"
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
