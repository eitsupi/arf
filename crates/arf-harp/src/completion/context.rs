//! Pure string analysis for completion context detection.

/// Context for package name completion.
#[derive(Debug, PartialEq)]
pub enum PackageContext {
    /// Inside library() or require() - suggest package names without `::`
    Library(String),
    /// Typing a potential package name - suggest with `::` suffix
    Namespace(String),
    /// No package context
    None,
}

/// Detect the package completion context.
///
/// Returns the context type and partial package name being typed.
pub fn detect_package_context(line: &str, cursor_pos: usize) -> PackageContext {
    // First check for library()/require() context
    if let Some(partial) = detect_library_context(line, cursor_pos) {
        return PackageContext::Library(partial);
    }

    // Then check for namespace context (typing a token that could be a package name)
    if let Some(partial) = detect_namespace_context(line, cursor_pos) {
        return PackageContext::Namespace(partial);
    }

    PackageContext::None
}

/// Check if the cursor is inside a library() or require() call.
///
/// Returns the partial package name being typed if inside such a call, None otherwise.
fn detect_library_context(line: &str, cursor_pos: usize) -> Option<String> {
    let before_cursor = &line[..cursor_pos.min(line.len())];

    // Find the last opening parenthesis before cursor
    let mut paren_depth = 0;
    let mut last_open_paren_pos = None;

    for (i, c) in before_cursor.char_indices().rev() {
        match c {
            ')' => paren_depth += 1,
            '(' => {
                if paren_depth == 0 {
                    last_open_paren_pos = Some(i);
                    break;
                }
                paren_depth -= 1;
            }
            _ => {}
        }
    }

    let open_pos = last_open_paren_pos?;

    // Check if the text before '(' is 'library' or 'require'
    let before_paren = before_cursor[..open_pos].trim_end();
    let func_name = before_paren
        .rsplit(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
        .next()?;

    if func_name != "library" && func_name != "require" {
        return None;
    }

    // Extract the partial package name after '('
    let after_paren = &before_cursor[open_pos + 1..];

    // Check if there's already a comma (additional arguments), then we're past the package name
    if after_paren.contains(',') {
        return None;
    }

    // Get the token being typed (unquoted package name)
    let trimmed = after_paren.trim_start();

    // Skip if it starts with a quote (string argument)
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        return None;
    }

    // Extract the identifier being typed
    let partial: String = trimmed
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_')
        .collect();

    Some(partial)
}

/// Check if the cursor is at the end of a potential package name token.
///
/// Returns the token if it could be a package name for namespace access (pkg::).
/// Returns None if:
/// - Inside a library()/require() call (handled separately)
/// - Inside a string
/// - The token contains `::`
/// - No valid identifier token at cursor
fn detect_namespace_context(line: &str, cursor_pos: usize) -> Option<String> {
    let before_cursor = &line[..cursor_pos.min(line.len())];

    // Skip if we're inside a string
    if is_in_string(before_cursor) {
        return None;
    }

    // Skip if the token already contains `::`
    // (R's built-in completion handles `pkg::` context)
    if before_cursor.ends_with("::") || before_cursor.ends_with(":::") {
        return None;
    }

    // Extract the identifier token at cursor position
    let token = extract_identifier_before_cursor(before_cursor)?;

    // Skip empty tokens or very short ones (less useful for package completion)
    if token.is_empty() {
        return None;
    }

    // Skip if the token is part of a `pkg::` expression (already being completed)
    // Check if there's a `::` right after the cursor in the original line
    let after_cursor = &line[cursor_pos..];
    if after_cursor.starts_with("::") || after_cursor.starts_with(":::") {
        return None;
    }

    Some(token)
}

/// Check if the cursor is inside a string literal.
///
/// This is a simple heuristic that counts unescaped quotes.
fn is_in_string(before_cursor: &str) -> bool {
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut chars = before_cursor.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // Skip escaped character
                chars.next();
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            _ => {}
        }
    }

    in_double_quote || in_single_quote
}

/// Extract the identifier token immediately before the cursor.
///
/// Returns the identifier if the cursor is at the end of one.
fn extract_identifier_before_cursor(before_cursor: &str) -> Option<String> {
    // Collect characters that form a valid R identifier (backwards from cursor)
    let token: String = before_cursor
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_')
        .collect::<String>()
        .chars()
        .rev()
        .collect();

    if token.is_empty() {
        return None;
    }

    // R identifiers can't start with a digit (unless backtick-quoted, which we ignore)
    let first_char = token.chars().next()?;
    if first_char.is_ascii_digit() {
        return None;
    }

    Some(token)
}

/// Check if the text contains a namespace operator (:: or :::).
pub(super) fn contains_namespace_operator(text: &str) -> bool {
    text.contains("::")
}

/// Returns true if the cursor (end of `text`) is inside an unclosed `(` — i.e.,
/// there is at least one `(` with no matching `)` that is not inside a string
/// literal or comment. This covers both function calls (`str(aaa_`) and grouped
/// expressions (`x <- (aaa_`).
///
/// Uses a forward scan with lightweight string tracking (double/single quotes
/// and backslash escapes). Unmatched `)` are treated as no-ops (depth clamped
/// at 0) so that expressions like `1) + str(aaa_` are correctly detected as
/// being inside `str(`.
pub(super) fn has_unmatched_open_paren(text: &str) -> bool {
    let mut in_double = false;
    let mut in_single = false;
    let mut in_comment = false;
    let mut escaped = false;
    let mut depth = 0i32;

    for c in text.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        // Comment runs to end of line only; resume scanning on the next line.
        if in_comment {
            if c == '\n' {
                in_comment = false;
            }
            continue;
        }
        match c {
            '#' if !in_double && !in_single => in_comment = true,
            '\\' if in_double || in_single => escaped = true,
            '"' if !in_single => in_double = !in_double,
            '\'' if !in_double => in_single = !in_single,
            '(' if !in_double && !in_single => depth += 1,
            ')' if !in_double && !in_single => depth = (depth - 1).max(0),
            _ => {}
        }
    }

    depth > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_package_context_library() {
        // Inside library()
        assert_eq!(
            detect_package_context("library(", 8),
            PackageContext::Library("".to_string())
        );
        assert_eq!(
            detect_package_context("library(dpl", 11),
            PackageContext::Library("dpl".to_string())
        );
        assert_eq!(
            detect_package_context("library(ggplot2", 15),
            PackageContext::Library("ggplot2".to_string())
        );
    }

    #[test]
    fn test_detect_package_context_require() {
        // Inside require()
        assert_eq!(
            detect_package_context("require(", 8),
            PackageContext::Library("".to_string())
        );
        assert_eq!(
            detect_package_context("require(tid", 11),
            PackageContext::Library("tid".to_string())
        );
    }

    #[test]
    fn test_detect_package_context_with_spaces() {
        // With spaces
        assert_eq!(
            detect_package_context("library( dpl", 12),
            PackageContext::Library("dpl".to_string())
        );
        assert_eq!(
            detect_package_context("  library(gg", 12),
            PackageContext::Library("gg".to_string())
        );
    }

    #[test]
    fn test_detect_package_context_library_edge_cases() {
        // After comma (additional arguments) - not library context, but namespace context
        assert_eq!(
            detect_package_context("library(dplyr, ", 15),
            PackageContext::None
        );

        // With quoted string - not package context
        assert_eq!(
            detect_package_context(r#"library("dplyr"#, 14),
            PackageContext::None
        );
        assert_eq!(
            detect_package_context("library('dplyr", 14),
            PackageContext::None
        );

        // Just "library" without paren - namespace context
        assert_eq!(
            detect_package_context("library", 7),
            PackageContext::Namespace("library".to_string())
        );
    }

    #[test]
    fn test_detect_package_context_nested() {
        // Nested parentheses - cursor in outer library()
        assert_eq!(
            detect_package_context("library(dpl", 11),
            PackageContext::Library("dpl".to_string())
        );
    }

    // Tests for namespace context (pkg:: completion)
    #[test]
    fn test_detect_namespace_context_basic() {
        // Simple identifier - should suggest pkg::
        assert_eq!(
            detect_package_context("sta", 3),
            PackageContext::Namespace("sta".to_string())
        );
        assert_eq!(
            detect_package_context("ggplot", 6),
            PackageContext::Namespace("ggplot".to_string())
        );
    }

    #[test]
    fn test_detect_namespace_context_in_expression() {
        // In an assignment
        assert_eq!(
            detect_package_context("x <- sta", 8),
            PackageContext::Namespace("sta".to_string())
        );
        // After operator
        assert_eq!(
            detect_package_context("1 + bas", 7),
            PackageContext::Namespace("bas".to_string())
        );
    }

    #[test]
    fn test_detect_namespace_context_not_in_string() {
        // Inside double-quoted string - no namespace context
        assert_eq!(detect_package_context(r#""sta"#, 4), PackageContext::None);
        assert_eq!(
            detect_package_context(r#"x <- "sta"#, 9),
            PackageContext::None
        );
        // Inside single-quoted string
        assert_eq!(detect_package_context("'sta", 4), PackageContext::None);
    }

    #[test]
    fn test_file_path_context_in_string() {
        // File paths in strings should return PackageContext::None
        // so that R's built-in completion handles file path completion
        assert_eq!(
            detect_package_context(r#"read.csv("./data/"#, 17),
            PackageContext::None
        );
        assert_eq!(
            detect_package_context(r#"source("myfile.R"#, 16),
            PackageContext::None
        );
        assert_eq!(
            detect_package_context("load('data.rda", 14),
            PackageContext::None
        );
        // Tilde expansion paths
        assert_eq!(
            detect_package_context(r#"setwd("~/Documents/"#, 19),
            PackageContext::None
        );
        // Absolute paths
        assert_eq!(
            detect_package_context(r#"file.exists("/home/user/"#, 24),
            PackageContext::None
        );
    }

    #[test]
    fn test_detect_namespace_context_after_colons() {
        // After :: - R's built-in handles this
        assert_eq!(detect_package_context("stats::", 7), PackageContext::None);
        assert_eq!(detect_package_context("stats:::", 8), PackageContext::None);
        // Inside existing pkg::func - don't suggest pkg:: again
        // (cursor at position 5 means "stats" with "::" following)
        assert_eq!(
            detect_package_context("stats::filter", 5),
            PackageContext::None
        );
    }

    #[test]
    fn test_detect_namespace_context_no_identifier() {
        // No identifier at cursor
        assert_eq!(detect_package_context("x <- ", 5), PackageContext::None);
        assert_eq!(detect_package_context("", 0), PackageContext::None);
        // Just operators/punctuation
        assert_eq!(detect_package_context("(", 1), PackageContext::None);
    }

    #[test]
    fn test_detect_namespace_context_numeric() {
        // Starts with digit - not a valid identifier
        assert_eq!(detect_package_context("123abc", 6), PackageContext::None);
    }

    #[test]
    fn test_is_in_string() {
        assert!(!is_in_string("hello"));
        assert!(is_in_string(r#""hello"#));
        assert!(!is_in_string(r#""hello""#));
        assert!(is_in_string("'hello"));
        assert!(!is_in_string("'hello'"));
        // Escaped quotes
        assert!(is_in_string(r#""he\"llo"#));
        assert!(!is_in_string(r#""he\"llo""#));
    }

    #[test]
    fn test_extract_identifier() {
        assert_eq!(
            extract_identifier_before_cursor("stats"),
            Some("stats".to_string())
        );
        assert_eq!(
            extract_identifier_before_cursor("x <- stats"),
            Some("stats".to_string())
        );
        assert_eq!(
            extract_identifier_before_cursor("my.package"),
            Some("my.package".to_string())
        );
        assert_eq!(
            extract_identifier_before_cursor("my_func"),
            Some("my_func".to_string())
        );
        assert_eq!(extract_identifier_before_cursor(""), None);
        assert_eq!(extract_identifier_before_cursor("123"), None);
        assert_eq!(extract_identifier_before_cursor("x <- "), None);
    }

    #[test]
    fn test_has_unmatched_open_paren() {
        // Inside a function call (cursor before closing paren)
        assert!(has_unmatched_open_paren("str(aaa_"));
        assert!(has_unmatched_open_paren("foo(x ="));
        assert!(has_unmatched_open_paren("foo(x, y ="));
        // Cursor after comma: e.g. full line "foo(x,)" with cursor at pos 6
        assert!(has_unmatched_open_paren("foo(x,"));

        // Nested: cursor inside outer call, inner call already closed
        // e.g. "foo(x = bar()" → outer ( unmatched
        assert!(has_unmatched_open_paren("foo(x = bar()"));

        // Extra `)` earlier in line: depth is clamped at 0 so the `str(` is still found.
        assert!(has_unmatched_open_paren("1) + str(aaa_"));

        // Top-level: no open paren
        assert!(!has_unmatched_open_paren("aaa_bbb"));
        assert!(!has_unmatched_open_paren(""));

        // Balanced parens (cursor after closing paren)
        assert!(!has_unmatched_open_paren("str(aaa_)"));
        assert!(!has_unmatched_open_paren("foo(x = bar())"));

        // Parens inside string literals are ignored
        assert!(!has_unmatched_open_paren(r#"x <- "("; aaa_"#));
        assert!(!has_unmatched_open_paren(r#""("#));
        assert!(has_unmatched_open_paren(r#"paste("(", x"#)); // cursor inside paste()
        assert!(has_unmatched_open_paren("paste('(', x")); // single-quoted string

        // Paren in single-line comment is ignored
        assert!(!has_unmatched_open_paren("# str(aaa_"));

        // Multiline: comment on first line must not swallow subsequent lines
        assert!(has_unmatched_open_paren("# note (\nstr(aaa_"));
        // Function call before comment, cursor on next line
        assert!(has_unmatched_open_paren("foo( # comment\naaa_"));
        // Comment-only first line, no function call after
        assert!(!has_unmatched_open_paren("# note (\naaa_"));
    }
}
