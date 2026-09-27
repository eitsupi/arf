//! History schema documentation for SQLite-backed command history.
//!
//! This module provides functions to display the history database schema
//! and example R code for accessing it, used by both CLI and REPL commands.

use super::{PagerAction, PagerConfig, PagerContent, copy_to_clipboard, run};
use crate::config::{HistoryLocationSource, ResolvedHistoryLocation};
use crate::highlighter::RTreeSitterHighlighter;
use crate::pager::style_convert::styled_text_to_line;
use crossterm::event::{KeyCode, KeyModifiers};
use nu_ansi_term::{Color, Style};
use ratatui::style::{Color as RatColor, Modifier, Style as RatStyle};
use ratatui::text::{Line, Span};
use reedline::Highlighter;
use std::cell::{Cell, RefCell};
use std::io::{self, IsTerminal};
use std::path::Path;

/// Error returned when history directory cannot be determined.
#[derive(Debug)]
pub struct HistoryDirError;

impl std::fmt::Display for HistoryDirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "No persistent history directory is available")
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
pub fn print_schema(location: &ResolvedHistoryLocation) -> Result<(), HistoryDirError> {
    if location.source() == HistoryLocationSource::Volatile {
        return Err(HistoryDirError);
    }
    let history_path = location.directory().ok_or(HistoryDirError)?;

    // Check if stdout is a terminal - only use colors if it is
    if io::stdout().is_terminal() {
        print_schema_colored(history_path);
    } else {
        print_schema_plain(history_path);
    }

    Ok(())
}

/// Print schema with ANSI colors (for terminal output).
fn print_schema_colored(history_path: &Path) {
    let s = SchemaStyles::default();
    let r_path = history_path.join("r.db");
    let shell_path = history_path.join("shell.db");

    // Title
    println!("{}", s.heading.paint("# History Database"));
    println!();

    // Location section
    println!("{}", s.heading.paint("## Location"));
    println!();
    println!("- R mode: {}", s.path.paint(r_path.display().to_string()));
    println!(
        "- Shell mode: {}",
        s.path.paint(shell_path.display().to_string())
    );
    println!();

    // SQLite Schema section
    print_sql_schema(&s);
    println!();

    // Indexes section
    print_indexes(&s);
    println!();

    // arf artifact metadata section
    print_artifact_metadata(&s);
    println!();

    // R example code
    print_r_example_code(&s, &r_path);
}

/// Print schema as plain text (for piped output).
fn print_schema_plain(history_path: &Path) {
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
pub fn show_schema_pager(location: &ResolvedHistoryLocation) -> Result<(), HistoryDirError> {
    if location.source() == HistoryLocationSource::Volatile {
        return Err(HistoryDirError);
    }
    let history_path = location.directory().ok_or(HistoryDirError)?;

    // Generate content lines
    let lines = generate_schema_lines(history_path);
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
fn generate_schema_lines(history_path: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let r_path = history_path.join("r.db");
    let shell_path = history_path.join("shell.db");

    // Title
    lines.push("# History Database".to_string());
    lines.push(String::new());

    // Location section
    lines.push("## Location".to_string());
    lines.push(String::new());
    lines.push(format!("- R mode: {}", r_path.display()));
    lines.push(format!("- Shell mode: {}", shell_path.display()));
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

    // arf artifact metadata section
    lines.push("## arf Artifact Metadata".to_string());
    lines.push(String::new());
    lines.push("Newly created single-history databases also contain:".to_string());
    lines.push("```sql".to_string());
    lines.push("CREATE TABLE arf_metadata (".to_string());
    lines.push("    key   TEXT PRIMARY KEY NOT NULL,".to_string());
    lines.push("    value TEXT NOT NULL".to_string());
    lines.push(");".to_string());
    lines.push("```".to_string());
    lines.push("- `artifact`: `history`".to_string());
    lines.push("- `format_version`: `1`".to_string());
    lines.push("- `history_kind`: `r` or `shell`".to_string());
    lines.push("- `created_by_version`: the arf package version".to_string());
    lines.push(
        "Unified exports use `artifact = history-export`, `format_version = 1`, and `created_by_version`; they omit `history_kind`.".to_string(),
    );
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
    lines.push(format!("  {}", format_r_path(&r_path)));
    lines.push(")".to_string());
    lines.push("history_data <- dbGetQuery(".to_string());
    lines.push("  con,".to_string());
    lines.push(r#"  "SELECT * FROM history ORDER BY id DESC LIMIT 10""#.to_string());
    lines.push(") |>".to_string());
    lines.push("  as_tibble()".to_string());
    lines.push("dbDisconnect(con)".to_string());
    lines.push("```".to_string());

    lines
}

/// Format a filesystem path as a quoted, portable R string literal.
fn format_r_path(path: &Path) -> String {
    #[cfg(windows)]
    let portable_path = path.to_string_lossy().replace('\\', "/");
    #[cfg(not(windows))]
    let portable_path = path.to_string_lossy().into_owned();

    let mut escaped = String::with_capacity(portable_path.len());
    for character in portable_path.chars() {
        match character {
            '"' => escaped.push_str(r#"\""#),
            '\\' => escaped.push_str(r#"\\"#),
            '\n' => escaped.push_str(r#"\n"#),
            '\r' => escaped.push_str(r#"\r"#),
            '\t' => escaped.push_str(r#"\t"#),
            character => escaped.push(character),
        }
    }
    format!(r#""{escaped}""#)
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

/// Print the arf-owned artifact metadata schema and keys.
fn print_artifact_metadata(s: &SchemaStyles) {
    println!("{}", s.heading.paint("## arf Artifact Metadata"));
    println!();
    println!("Newly created single-history databases also contain:");
    println!("{}", s.code_fence.paint("```sql"));
    println!(
        "{} {} {} (",
        s.sql_keyword.paint("CREATE"),
        s.sql_keyword.paint("TABLE"),
        s.sql_identifier.paint("arf_metadata")
    );
    println!(
        "    {}   {} {},",
        s.sql_identifier.paint("key"),
        s.sql_keyword.paint("TEXT"),
        s.sql_keyword.paint("PRIMARY KEY NOT NULL")
    );
    println!(
        "    {} {} {}",
        s.sql_identifier.paint("value"),
        s.sql_keyword.paint("TEXT"),
        s.sql_keyword.paint("NOT NULL")
    );
    println!(");");
    println!("{}", s.code_fence.paint("```"));
    println!("- `artifact`: `history`");
    println!("- `format_version`: `1`");
    println!("- `history_kind`: `r` or `shell`");
    println!("- `created_by_version`: the arf package version");
    println!(
        "Unified exports use `artifact = history-export`, `format_version = 1`, and `created_by_version`; they omit `history_kind`."
    );
}

/// Print example R code for accessing the history database.
fn print_r_example_code(s: &SchemaStyles, r_path: &Path) {
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
    println!("  {}", s.r_string.paint(format_r_path(r_path)));
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
            .paint(r#""SELECT * FROM history ORDER BY id DESC LIMIT 10""#)
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
#[path = "history_schema/tests.rs"]
mod tests;
