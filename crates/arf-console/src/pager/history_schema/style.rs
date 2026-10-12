//! Ratatui styling of schema documentation lines.

use crate::highlighter::RTreeSitterHighlighter;
use crate::pager::style_convert::styled_text_to_line;
use ratatui::style::{Color as RatColor, Modifier, Style as RatStyle};
use ratatui::text::{Line, Span};
use reedline::Highlighter;
use std::cell::RefCell;

/// Track if we're inside a code block and what type.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum CodeBlockType {
    #[default]
    None,
    Sql,
    R,
}

/// State for tracking code block context during rendering.
#[derive(Clone, Copy, Default)]
pub(super) struct StyleState {
    pub(super) code_block: CodeBlockType,
}

impl StyleState {
    pub(super) fn new() -> Self {
        Self::default()
    }

    pub(super) fn update(&mut self, line: &str) {
        if line == "```sql" {
            self.code_block = CodeBlockType::Sql;
        } else if line == "```r" {
            self.code_block = CodeBlockType::R;
        } else if line == "```" {
            self.code_block = CodeBlockType::None;
        }
    }
}

thread_local! {
    /// Thread-local tree-sitter R highlighter for schema display.
    static R_HIGHLIGHTER: RefCell<RTreeSitterHighlighter> = RefCell::new(RTreeSitterHighlighter::default());
}

// --- ratatui-based style functions (used by PagerContent::render_line) ---

/// Apply syntax highlighting to a line, returning a ratatui `Line`.
pub(super) fn style_line_to_ratatui(line: &str, state: &StyleState) -> Line<'static> {
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
pub(super) fn style_sql_line_ratatui(line: &str) -> Line<'static> {
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
pub(super) fn style_r_line_ratatui(line: &str) -> Line<'static> {
    R_HIGHLIGHTER.with(|highlighter| {
        let styled = highlighter.borrow().highlight(line, 0);
        styled_text_to_line(&styled)
    })
}

/// Style a path line for ratatui rendering.
pub(super) fn style_path_line_ratatui(line: &str) -> Line<'static> {
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
pub(super) fn style_index_line_ratatui(line: &str) -> Line<'static> {
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
