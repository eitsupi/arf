//! Integration tests for R help rendering via rd2qmd.

mod common;

use arf_harp::get_help_markdown;
use common::{ld_library_path_is_set, with_r};

/// Regression test for GitHub issue #194:
/// `base::solve` contains `%*%` in its Rd source. Without `deparse = TRUE`,
/// `as.character()` emits unescaped `%` which rd-parser treats as a comment,
/// losing closing braces and producing a parse error. With `deparse = TRUE`
/// the `%` is escaped as `\%` and rd2qmd parses the page correctly.
#[cfg(target_os = "linux")]
#[test]
fn test_help_base_solve_returns_content() {
    if !ld_library_path_is_set() {
        eprintln!(
            "Skipping test_help_base_solve_returns_content: \
             LD_LIBRARY_PATH not set."
        );
        return;
    }

    let Some(()) = with_r(|| {
        let result = get_help_markdown("solve", Some("base"));
        match &result {
            Err(e) => panic!(r#"get_help_markdown("solve", Some("base")) failed: {e}"#),
            Ok(md) => {
                assert!(!md.is_empty(), "help markdown must not be empty");
                // The title of the help page is "Solve a System of Equations"
                assert!(
                    md.contains("Solve"),
                    "expected 'Solve' in help markdown, got:\n{md}"
                );
                // Regression check for rd-parser fix: \dots must be emitted as
                // ellipses rather than being swallowed by a mis-terminated macro name.
                assert!(
                    md.contains("...") || md.contains('\u{2026}'),
                    "expected ellipses in help markdown (\\dots regression), got:\n{md}"
                );
            }
        }
    }) else {
        panic!("R initialization failed; cannot run test_help_base_solve_returns_content");
    };
}
