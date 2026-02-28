//! R code completion using utils package internal functions.
//!
//! This module provides completion functionality by calling R's built-in
//! completion functions from the utils package.
//!
//! # Supported completion types
//!
//! - **Variables and functions**: Completes R objects in the global environment
//! - **Package names**: In `library()` and `require()` calls
//! - **Namespace access**: Suggests `package::` when typing potential package names
//! - **File paths**: Inside string literals (e.g., `read.csv("./data/`)
//! - **Function arguments**: Inside function calls
//!
//! File path completion works automatically inside quoted strings, using R's
//! built-in completion which supports relative paths, absolute paths, and
//! tilde expansion (`~`).

use crate::error::{HarpError, HarpResult};
use crate::protect::RProtect;
use arf_libr::{ParseStatus, SEXP, r_library, r_nil_value, restore_stderr, suppress_stderr};
use std::ffi::CString;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Guard that suppresses R stderr output and restores it on drop.
///
/// This is used during completion to prevent error messages from
/// interfering with the terminal display (especially on Windows).
/// This matches radian's suppress_stderr pattern - only stderr is suppressed,
/// stdout continues to work normally.
struct SuppressStderrGuard;

impl SuppressStderrGuard {
    fn new() -> Self {
        suppress_stderr();
        SuppressStderrGuard
    }
}

impl Drop for SuppressStderrGuard {
    fn drop(&mut self) {
        restore_stderr();
    }
}

/// Cache for installed packages.
struct PackageCache {
    packages: Vec<String>,
    last_updated: Option<Instant>,
}

impl PackageCache {
    const fn new() -> Self {
        PackageCache {
            packages: Vec::new(),
            last_updated: None,
        }
    }
}

static PACKAGE_CACHE: Mutex<PackageCache> = Mutex::new(PackageCache::new());

/// Cache duration for installed packages (5 minutes).
const CACHE_DURATION: Duration = Duration::from_secs(300);

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

/// Get the list of installed packages with caching.
fn get_installed_packages() -> HarpResult<Vec<String>> {
    // Check cache first
    if let Ok(cache) = PACKAGE_CACHE.lock()
        && let Some(last_updated) = cache.last_updated
        && last_updated.elapsed() < CACHE_DURATION
        && !cache.packages.is_empty()
    {
        return Ok(cache.packages.clone());
    }

    // Fetch from R
    let packages = fetch_installed_packages()?;

    // Update cache
    if let Ok(mut cache) = PACKAGE_CACHE.lock() {
        cache.packages = packages.clone();
        cache.last_updated = Some(Instant::now());
    }

    Ok(packages)
}

/// Get package completions for a partial package name (for library()/require()).
fn get_package_completions(partial: &str) -> HarpResult<Vec<String>> {
    let packages = get_installed_packages()?;

    let completions: Vec<String> = packages
        .into_iter()
        .filter(|pkg| pkg.starts_with(partial))
        .collect();

    Ok(completions)
}

/// Get package completions with `::` suffix (for namespace access).
fn get_namespace_completions(partial: &str) -> HarpResult<Vec<String>> {
    let packages = get_installed_packages()?;

    let completions: Vec<String> = packages
        .into_iter()
        .filter(|pkg| pkg.starts_with(partial))
        .map(|pkg| format!("{}::", pkg))
        .collect();

    Ok(completions)
}

/// Fetch installed packages from R using .packages(all.available = TRUE).
fn fetch_installed_packages() -> HarpResult<Vec<String>> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        let code = r#"
            tryCatch({
                .packages(all.available = TRUE)
            }, error = function(e) character(0))
        "#;

        let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
            expected: "valid UTF-8".to_string(),
            actual: "string with null byte".to_string(),
        })?;

        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        // Parse the code
        let mut status = ParseStatus::Null;
        let parsed = protect.protect((lib.r_parsevector)(
            code_sexp,
            -1,
            &mut status,
            r_nil_value()?,
        ));

        if status != ParseStatus::Ok {
            return Ok(vec![]);
        }

        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Ok(vec![]);
        }

        let expr = (lib.vector_elt)(parsed, 0);
        let global_env = *lib.r_globalenv;

        let mut payload = EvalPayload {
            expr,
            env: global_env,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 || payload.result.is_none() {
            return Ok(vec![]);
        }

        let result = protect.protect(payload.result.unwrap());

        extract_string_vector(result)
    }
}

/// Get the names from a package's namespace.
///
/// For `::` access (`triple_colon = false`), returns exported names via
/// `getNamespaceExports()`. For `:::` access (`triple_colon = true`),
/// returns all namespace objects (including internals) via `ls(asNamespace(), all.names = TRUE)`.
///
/// Returns an empty vector if the R evaluation fails (e.g., package not
/// installed). May return `Err` if the R runtime itself is unavailable.
pub fn get_namespace_exports(pkg: &str, triple_colon: bool) -> HarpResult<Vec<String>> {
    let _guard = SuppressStderrGuard::new();

    let lib = r_library()?;
    let mut protect = RProtect::new();

    // For `:::`, list all namespace objects (including internals).
    // For `::`, only exported names.
    let code = if triple_colon {
        format!(
            r#"
            tryCatch({{
                ls(asNamespace("{pkg}"), all.names = TRUE)
            }}, error = function(e) character(0))
            "#,
            pkg = escape_r_string(pkg),
        )
    } else {
        format!(
            r#"
            tryCatch({{
                getNamespaceExports("{pkg}")
            }}, error = function(e) character(0))
            "#,
            pkg = escape_r_string(pkg),
        )
    };

    unsafe {
        let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
            expected: "string without null bytes".to_string(),
            actual: "string with null byte".to_string(),
        })?;

        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        let mut status = ParseStatus::Null;
        let parsed = protect.protect((lib.r_parsevector)(
            code_sexp,
            -1,
            &mut status,
            r_nil_value()?,
        ));

        if status != ParseStatus::Ok {
            return Ok(vec![]);
        }

        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Ok(vec![]);
        }

        let expr = (lib.vector_elt)(parsed, 0);
        let global_env = *lib.r_globalenv;

        let mut payload = EvalPayload {
            expr,
            env: global_env,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 || payload.result.is_none() {
            return Ok(vec![]);
        }

        let result = protect.protect(payload.result.unwrap());

        extract_string_vector(result)
    }
}

/// Get completions for the given line at the specified cursor position.
///
/// Returns a list of completion candidates.
///
/// # Arguments
/// * `line` - The input line
/// * `cursor_pos` - Cursor position in the line
/// * `timeout_ms` - Timeout in milliseconds for R completion (0 = no timeout)
pub fn get_completions(line: &str, cursor_pos: usize, timeout_ms: u64) -> HarpResult<Vec<String>> {
    // Suppress R console output during completion to prevent error messages
    // from interfering with the terminal display (especially on Windows).
    let _guard = SuppressStderrGuard::new();

    // Check for package context first
    match detect_package_context(line, cursor_pos) {
        PackageContext::Library(partial) => {
            // Inside library()/require() - return package names without `::`
            return get_package_completions(&partial);
        }
        PackageContext::Namespace(partial) => {
            // Typing a potential package name - return packages with `::`
            // Also combine with R's built-in completions
            let mut completions = get_namespace_completions(&partial)?;
            // Add R's built-in completions (for variables, functions, etc.)
            // Filter out `pkg::` completions since we already have them from get_namespace_completions
            if let Ok(r_completions) = get_r_builtin_completions(line, cursor_pos, timeout_ms) {
                completions.extend(r_completions.into_iter().filter(|c| !c.ends_with("::")));
            }
            return Ok(completions);
        }
        PackageContext::None => {
            // No package context - use R's built-in completions only
        }
    }

    // Determine effective timeout:
    // - Disable timeout for package::func completions (they need full results and are worth waiting for)
    // - This matches radian's behavior
    let effective_timeout = if contains_namespace_operator(&line[..cursor_pos.min(line.len())]) {
        0 // No timeout for :: completions
    } else {
        timeout_ms
    };

    get_r_builtin_completions(line, cursor_pos, effective_timeout)
}

/// Check if the text contains a namespace operator (:: or :::).
fn contains_namespace_operator(text: &str) -> bool {
    text.contains("::")
}

/// Get R's built-in completions using utils package functions.
///
/// # Arguments
/// * `line` - The input line
/// * `cursor_pos` - Cursor position in the line
/// * `timeout_ms` - Timeout in milliseconds (0 = no timeout)
///
/// Uses `base::setTimeLimit()` to prevent slow completions from blocking the UI.
/// This is similar to the approach used in radian.
fn get_r_builtin_completions(
    line: &str,
    cursor_pos: usize,
    timeout_ms: u64,
) -> HarpResult<Vec<String>> {
    let lib = r_library()?;
    let mut protect = RProtect::new();

    // Convert timeout to seconds for R's setTimeLimit
    let timeout_secs = timeout_ms as f64 / 1000.0;
    let use_timeout = timeout_ms > 0;

    unsafe {
        // Build R code to call completion functions
        // Note: .guessTokenFromLine() must be called before .completeToken()
        // to set the token in .CompletionEnv
        //
        // When timeout is enabled, wrap completeToken() with setTimeLimit()
        // to prevent slow completions from blocking the UI.
        // Use both cpu and elapsed time limits for better coverage.
        // transient = TRUE makes the limit apply only to this expression.
        let code = format!(
            r#"
            local({{
                utils:::.assignLinebuffer("{line}")
                utils:::.assignEnd({cursor_pos}L)
                utils:::.guessTokenFromLine()
                tryCatch({{
                    if ({use_timeout}) base::setTimeLimit(cpu = {timeout}, elapsed = {timeout}, transient = TRUE)
                    utils:::.completeToken()
                    if ({use_timeout}) base::setTimeLimit(cpu = Inf, elapsed = Inf, transient = FALSE)
                    utils:::.retrieveCompletions()
                }}, error = function(e) {{
                    if ({use_timeout}) base::setTimeLimit(cpu = Inf, elapsed = Inf, transient = FALSE)
                    character(0)
                }})
            }})
            "#,
            line = escape_r_string(line),
            cursor_pos = cursor_pos,
            use_timeout = if use_timeout { "TRUE" } else { "FALSE" },
            timeout = timeout_secs,
        );

        let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
            expected: "valid UTF-8".to_string(),
            actual: "string with null byte".to_string(),
        })?;

        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        // Parse the code
        let mut status = ParseStatus::Null;
        let parsed = protect.protect((lib.r_parsevector)(
            code_sexp,
            -1,
            &mut status,
            r_nil_value()?,
        ));

        if status != ParseStatus::Ok {
            return Ok(vec![]);
        }

        // Get the first expression
        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Ok(vec![]);
        }

        let expr = (lib.vector_elt)(parsed, 0);
        let global_env = *lib.r_globalenv;

        // Evaluate using R_ToplevelExec for safe error handling
        let mut payload = EvalPayload {
            expr,
            env: global_env,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 || payload.result.is_none() {
            return Ok(vec![]);
        }

        let result = protect.protect(payload.result.unwrap());

        // Convert result to Vec<String>
        extract_string_vector(result)
    }
}

/// Get the token being completed from the line.
pub fn get_token(line: &str, cursor_pos: usize) -> HarpResult<String> {
    // Suppress R console output during token extraction
    let _guard = SuppressStderrGuard::new();

    let lib = r_library()?;
    let mut protect = RProtect::new();

    unsafe {
        let code = format!(
            r#"
            local({{
                utils:::.assignLinebuffer("{}")
                utils:::.assignEnd({}L)
                utils:::.guessTokenFromLine()
            }})
            "#,
            escape_r_string(line),
            cursor_pos
        );

        let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
            expected: "valid UTF-8".to_string(),
            actual: "string with null byte".to_string(),
        })?;

        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        // Parse and evaluate
        let mut status = ParseStatus::Null;
        let parsed = protect.protect((lib.r_parsevector)(
            code_sexp,
            -1,
            &mut status,
            r_nil_value()?,
        ));

        if status != ParseStatus::Ok {
            return Ok(String::new());
        }

        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Ok(String::new());
        }

        let expr = (lib.vector_elt)(parsed, 0);
        let global_env = *lib.r_globalenv;

        let mut payload = EvalPayload {
            expr,
            env: global_env,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 || payload.result.is_none() {
            return Ok(String::new());
        }

        let result = protect.protect(payload.result.unwrap());

        // Extract single string
        extract_single_string(result)
    }
}

/// Payload for R_ToplevelExec callback.
struct EvalPayload {
    expr: SEXP,
    env: SEXP,
    result: Option<SEXP>,
}

/// Callback for R_ToplevelExec - evaluates the expression.
unsafe extern "C" fn eval_callback(payload: *mut std::ffi::c_void) {
    let data = unsafe { &mut *(payload as *mut EvalPayload) };
    let lib = match r_library() {
        Ok(lib) => lib,
        Err(_) => return,
    };
    let result = unsafe { (lib.rf_eval)(data.expr, data.env) };
    data.result = Some(result);
}

/// Extract a character vector to Vec<String>.
unsafe fn extract_string_vector(sexp: SEXP) -> HarpResult<Vec<String>> {
    let lib = r_library()?;

    unsafe {
        // Check if it's a string vector
        if (lib.rf_isstring)(sexp) == 0 {
            return Ok(vec![]);
        }

        let len = (lib.rf_length)(sexp) as isize;
        let mut result = Vec::with_capacity(len as usize);

        for i in 0..len {
            let elt = (lib.string_elt)(sexp, i);
            let cstr = (lib.r_charsxp)(elt);
            if !cstr.is_null()
                && let Ok(s) = std::ffi::CStr::from_ptr(cstr).to_str()
            {
                result.push(s.to_string());
            }
        }

        Ok(result)
    }
}

/// Extract a single string from SEXP.
unsafe fn extract_single_string(sexp: SEXP) -> HarpResult<String> {
    let lib = r_library()?;

    unsafe {
        if (lib.rf_isstring)(sexp) == 0 || (lib.rf_length)(sexp) == 0 {
            return Ok(String::new());
        }

        let elt = (lib.string_elt)(sexp, 0);
        let cstr = (lib.r_charsxp)(elt);
        if cstr.is_null() {
            return Ok(String::new());
        }

        match std::ffi::CStr::from_ptr(cstr).to_str() {
            Ok(s) => Ok(s.to_string()),
            Err(_) => Ok(String::new()),
        }
    }
}

/// Escape a string for use in R code.
fn escape_r_string(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// Check if the given names are functions in R.
///
/// This function checks multiple names efficiently in a single R call.
/// Returns a vector of booleans indicating whether each name is a function.
///
/// # Arguments
/// * `names` - The names to check
///
/// # Note
/// For performance, callers should limit the number of names checked.
/// Checking ~50 names takes <1ms, but thousands can take 100+ms.
pub fn check_if_functions(names: &[&str]) -> HarpResult<Vec<bool>> {
    if names.is_empty() {
        return Ok(vec![]);
    }

    // Suppress R console output during function checking
    let _guard = SuppressStderrGuard::new();

    let lib = r_library()?;
    let mut protect = RProtect::new();

    // Build R code to check all names at once
    // Using mode(get0(x, inherits=TRUE)) == "function" for each name
    let names_r: Vec<String> = names
        .iter()
        .map(|n| format!(r#""{}""#, escape_r_string(n)))
        .collect();
    let names_vector = names_r.join(", ");

    let code = format!(
        r#"
        local({{
            names <- c({})
            vapply(names, function(n) {{
                # Use eval(parse()) to handle both simple names and pkg::func syntax
                tryCatch({{
                    obj <- eval(parse(text = n))
                    is.function(obj)
                }}, error = function(e) FALSE)
            }}, logical(1), USE.NAMES = FALSE)
        }})
        "#,
        names_vector
    );

    unsafe {
        let code_cstring = CString::new(code).map_err(|_| HarpError::TypeMismatch {
            expected: "valid UTF-8".to_string(),
            actual: "string with null byte".to_string(),
        })?;

        let code_sexp = protect.protect((lib.rf_mkstring)(code_cstring.as_ptr()));

        // Parse the code
        let mut status = ParseStatus::Null;
        let parsed = protect.protect((lib.r_parsevector)(
            code_sexp,
            -1,
            &mut status,
            r_nil_value()?,
        ));

        if status != ParseStatus::Ok {
            return Ok(vec![false; names.len()]);
        }

        let n_expr = (lib.rf_length)(parsed);
        if n_expr == 0 {
            return Ok(vec![false; names.len()]);
        }

        let expr = (lib.vector_elt)(parsed, 0);
        let global_env = *lib.r_globalenv;

        let mut payload = EvalPayload {
            expr,
            env: global_env,
            result: None,
        };

        let success = (lib.r_toplevelexec)(
            Some(eval_callback),
            &mut payload as *mut EvalPayload as *mut std::ffi::c_void,
        );

        if success == 0 || payload.result.is_none() {
            return Ok(vec![false; names.len()]);
        }

        let result = protect.protect(payload.result.unwrap());

        // Extract logical vector
        extract_logical_vector(result, names.len())
    }
}

/// R's LGLSXP type code (logical vector).
const LGLSXP: i32 = 10;

/// Extract a logical vector to Vec<bool>.
unsafe fn extract_logical_vector(sexp: SEXP, expected_len: usize) -> HarpResult<Vec<bool>> {
    let lib = r_library()?;

    unsafe {
        // Check if it's a logical vector using TYPEOF
        if (lib.rf_typeof)(sexp) != LGLSXP {
            return Ok(vec![false; expected_len]);
        }

        let len = (lib.rf_length)(sexp) as usize;
        if len != expected_len {
            return Ok(vec![false; expected_len]);
        }

        let ptr = (lib.logical)(sexp);
        let mut result = Vec::with_capacity(len);

        for i in 0..len {
            // R's TRUE is 1, FALSE is 0, NA is INT_MIN
            let val = *ptr.add(i);
            result.push(val == 1);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_r_string() {
        assert_eq!(escape_r_string("hello"), "hello");
        assert_eq!(escape_r_string(r#"he"llo"#), r#"he\"llo"#);
        assert_eq!(escape_r_string("he\\llo"), "he\\\\llo");
        assert_eq!(escape_r_string("line1\nline2"), "line1\\nline2");
    }

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
}
