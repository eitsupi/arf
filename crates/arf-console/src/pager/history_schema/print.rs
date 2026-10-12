//! Colored and plain-text printing of the schema documentation.

use super::lines::{format_r_path, generate_schema_lines};
use nu_ansi_term::{Color, Style};
use std::path::Path;

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

/// Print schema with ANSI colors (for terminal output).
pub(super) fn print_schema_colored(history_path: &Path) {
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
pub(super) fn print_schema_plain(history_path: &Path) {
    // Use the same lines as generate_schema_lines for consistency
    for line in generate_schema_lines(history_path) {
        println!("{}", line);
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
