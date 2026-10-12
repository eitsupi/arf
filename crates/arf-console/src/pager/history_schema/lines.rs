//! Plain-text schema documentation lines.

use std::path::Path;

/// Generate the schema documentation as a vector of lines.
pub(super) fn generate_schema_lines(history_path: &Path) -> Vec<String> {
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
pub(super) fn format_r_path(path: &Path) -> String {
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

/// Extract the R code block content from schema lines.
///
/// Returns the lines between ```r and ```, excluding the fence markers.
pub(super) fn extract_r_code_block(lines: &[String]) -> String {
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
