//! Naming rules for R completions: backtick quoting and names that must not
//! receive call parentheses.

/// Check if an R name requires backtick quoting (non-syntactic name).
///
/// Names that start with an ASCII letter or `.` followed by a non-digit, and
/// contain only ASCII alphanumeric, `.`, or `_` characters are syntactic.
/// Everything else (operators like `%>%`, names starting with `_` or digits,
/// names with non-ASCII or special characters) requires backtick quoting.
///
/// This is intentionally conservative (ASCII-only) to match R's default
/// parser behavior where non-ASCII identifiers require backtick quoting.
pub(super) fn needs_backtick_quoting(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    let mut chars = name.chars();
    let first = chars.next().unwrap();

    // Must start with a letter or '.'
    if !first.is_ascii_alphabetic() && first != '.' {
        return true;
    }

    // If starts with '.', second char must not be a digit
    if first == '.'
        && let Some(second) = chars.next()
    {
        if second.is_ascii_digit() {
            return true;
        }
        // Check remaining chars after second
        if !second.is_ascii_alphanumeric() && second != '.' && second != '_' {
            return true;
        }
    }

    // All remaining characters must be alphanumeric, '.', or '_'
    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '.' && c != '_' {
            return true;
        }
    }

    false
}

/// Check if a completion has a special suffix that means it shouldn't get parentheses.
pub(super) fn has_special_suffix(s: &str) -> bool {
    // Package namespace (already has ::)
    s.ends_with("::")
        // Assignment operators
        || s.ends_with("<-")
        || s.ends_with("<<-")
        // Already has parentheses
        || s.ends_with("()")
        || s.ends_with('(')
        // File paths (R completion returns these for strings)
        || s.ends_with('/')
        // Argument names (completion inside function calls)
        || s.ends_with(" = ")
        || s.ends_with("=")
}
