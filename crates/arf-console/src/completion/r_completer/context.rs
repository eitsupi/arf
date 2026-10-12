//! Detection of the completion context around the cursor: the cache key for
//! the token being completed, and enclosing `library()`-style calls.

/// The cursor-adjacent token and the surrounding input used as a cache key.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct CompletionContext {
    pub(super) token: String,
    pub(super) prefix: String,
    pub(super) suffix: String,
}

/// Build a cache context only when the token is the exact suffix before the
/// cursor. Invalid UTF-8 boundaries and non-adjacent tokens are cache misses.
pub(super) fn completion_context(
    line: &str,
    cursor_pos: usize,
    token: &str,
) -> Option<CompletionContext> {
    let cursor_pos = cursor_pos.min(line.len());
    if !line.is_char_boundary(cursor_pos) {
        return None;
    }
    let token_start = cursor_pos.checked_sub(token.len())?;
    if !line.is_char_boundary(token_start) || &line[token_start..cursor_pos] != token {
        return None;
    }
    Some(CompletionContext {
        token: token.to_owned(),
        prefix: line[..token_start].to_owned(),
        suffix: line[cursor_pos..].to_owned(),
    })
}

/// Context for a detected `library(partial)` or similar call.
#[derive(Debug, PartialEq)]
pub(super) struct LibraryContext {
    /// The partial package name being typed (may be empty).
    pub(super) partial: String,
    /// Byte position where the partial starts (after `(` + whitespace).
    pub(super) start_pos: usize,
}

/// Detect if the cursor is inside a `library()`, `require()`, or user-configured function call.
///
/// Scans backwards from cursor to find the last unmatched `(`, extracts the function name,
/// and checks it against `func_names`. Returns `None` if:
/// - No unmatched `(` found
/// - Function name doesn't match any in `func_names`
/// - There's a comma after `(` (not first argument)
/// - The argument starts with a quote (string argument)
///
/// # Limitation
///
/// The backward parenthesis scan does not track string literal boundaries, so a `)` inside
/// a string literal is counted as a real closing paren. In theory this could miscount
/// paren depth, but in practice it is unreachable for `library()`/`require()` because:
/// - These functions take a bare symbol as the first argument, not nested expressions.
/// - Any nested call containing a string with `)` would also contain commas,
///   which triggers the early `None` return (first-argument-only guard).
/// - A `)` in a string *before* the target `(` is never reached, since the scan
///   stops at the nearest unmatched `(`.
///
/// The same approach is used in `arf-harp/src/completion/context.rs`.
pub(super) fn detect_library_context(
    line: &str,
    cursor_pos: usize,
    func_names: &[String],
) -> Option<LibraryContext> {
    let pos = cursor_pos.min(line.len());
    if !line.is_char_boundary(pos) {
        return None;
    }
    let before_cursor = &line[..pos];

    // Find the last unmatched opening parenthesis before cursor
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

    // Extract the function name before `(` (identifier chars, plus `::` for `box::use` style)
    let before_paren = before_cursor[..open_pos].trim_end();
    let func_name = before_paren
        .rsplit(|c: char| !c.is_alphanumeric() && c != '_' && c != '.' && c != ':')
        .next()?;

    if func_name.is_empty() {
        return None;
    }

    // Skip member access operators: obj$library( or env@require( are not real calls
    let func_start = before_paren.len() - func_name.len();
    if func_start > 0 {
        let preceding_char = before_paren[..func_start].chars().next_back();
        if matches!(preceding_char, Some('$' | '@')) {
            return None;
        }
    }

    // Check if it matches any configured function name
    if !func_names.iter().any(|f| f == func_name) {
        return None;
    }

    // Extract what's after the `(`
    let after_paren = &before_cursor[open_pos + 1..];

    // Skip if comma present (not first argument)
    if after_paren.contains(',') {
        return None;
    }

    let trimmed = after_paren.trim_start();

    // Skip if starts with a quote (string argument)
    if trimmed.starts_with('"') || trimmed.starts_with('\'') {
        return None;
    }

    // Extract the partial identifier
    let partial: String = trimmed
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '_')
        .collect();

    // Calculate start_pos: the byte position where the partial starts
    let whitespace_len = after_paren.len() - trimmed.len();
    let start_pos = open_pos + 1 + whitespace_len;

    Some(LibraryContext { partial, start_pos })
}
