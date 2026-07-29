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

mod context;
mod package_discovery;
mod r_ffi;

use crate::error::HarpResult;
use arf_libr::{restore_stderr, suppress_stderr};

pub use context::{PackageContext, detect_package_context};
pub use package_discovery::get_installed_packages;
pub use r_ffi::{check_if_functions, get_namespace_exports, get_token};

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
            return package_discovery::get_package_completions(&partial);
        }
        PackageContext::Namespace(partial) => {
            // Typing a potential package name - return packages with `::`
            // Also combine with R's built-in completions
            let mut completions = package_discovery::get_namespace_completions(&partial)?;
            // Add R's built-in completions (for variables, functions, etc.)
            // Filter out `pkg::` completions since we already have them from get_namespace_completions
            if let Ok(r_completions) =
                r_ffi::get_r_builtin_completions(line, cursor_pos, timeout_ms)
            {
                completions.extend(r_completions.into_iter().filter(|c| !c.ends_with("::")));
            }
            return Ok(completions);
        }
        PackageContext::None => {
            // No package context - use R's built-in completions only
        }
    }

    // Raise timeout for contexts where R's completer does extra work:
    // - `::` completions: enumerate package exports (slow)
    // - inside an unclosed `(` (function call or grouped expression): R also looks up argument
    //   names (~150ms vs ~20ms at top level)
    // Use a generous fixed floor (1000ms) so unusually slow environments still get
    // a safety boundary. timeout_ms=0 (no limit) is preserved as-is.
    let before_cursor = &line[..cursor_pos.min(line.len())];
    let effective_timeout = if timeout_ms == 0 {
        0
    } else if context::contains_namespace_operator(before_cursor)
        || context::has_unmatched_open_paren(before_cursor)
    {
        timeout_ms.max(1000)
    } else {
        timeout_ms
    };

    r_ffi::get_r_builtin_completions(line, cursor_pos, effective_timeout)
}
