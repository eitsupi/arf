//! Integration tests for R help rendering via rd2qmd.

// All tests in this file are Linux-only. Gate the entire module to avoid
// unused-import warnings on other platforms (e.g. Windows clippy -D warnings).
#![cfg(target_os = "linux")]

mod common;

use arf_harp::{get_help_markdown, get_help_topics, get_package_help_markdown};
use common::{ld_library_path_is_set, with_r};

/// Regression test for GitHub issue #194:
/// `base::solve` contains `%*%` in its Rd source. Without `deparse = TRUE`,
/// `as.character()` emits unescaped `%` which rd-parser treats as a comment,
/// losing closing braces and producing a parse error. With `deparse = TRUE`
/// the `%` is escaped as `\%` and rd2qmd parses the page correctly.
#[test]
fn test_help_base_solve_returns_content() {
    if !ld_library_path_is_set() {
        eprintln!(
            "Skipping test_help_base_solve_returns_content: \
             LD_LIBRARY_PATH not set."
        );
        return;
    }

    with_r(|| {
        arf_harp::lib_paths::populate_lib_paths().expect(".libPaths() should evaluate");
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
    });
}

#[test]
fn test_help_package_alias_and_exact_key() {
    if !ld_library_path_is_set() {
        eprintln!("Skipping test_help_package_alias_and_exact_key: LD_LIBRARY_PATH not set.");
        return;
    }

    with_r(|| {
        let alias = get_package_help_markdown("[.data.frame", "base")
            .expect("help lookup by alias should succeed");
        let exact_key = get_package_help_markdown("Extract.data.frame", "base")
            .expect("help lookup by exact key should succeed");

        assert!(!alias.is_empty(), "help for alias was empty");
        assert!(!exact_key.is_empty(), "help for exact key was empty");
        assert_eq!(alias, exact_key);
    });
}

#[test]
fn test_help_package_and_topic_not_found() {
    if !ld_library_path_is_set() {
        eprintln!("Skipping test_help_package_and_topic_not_found: LD_LIBRARY_PATH not set.");
        return;
    }

    with_r(|| {
        arf_harp::lib_paths::populate_lib_paths().expect(".libPaths() should evaluate");
        let package_error =
            get_package_help_markdown("solve", "definitely_not_an_installed_package")
                .expect_err("unknown package should fail");
        assert!(package_error.to_string().contains("not found"));

        let topic_error = get_package_help_markdown("definitely_not_a_help_topic", "base")
            .expect_err("unknown topic should fail");
        let message = topic_error.to_string();
        assert!(message.contains("definitely_not_a_help_topic"));
        assert!(message.contains("base"));
    });
}

#[test]
fn test_help_topics_reads_installed_indexes() {
    if !ld_library_path_is_set() {
        eprintln!("Skipping test_help_topics_reads_installed_indexes: LD_LIBRARY_PATH not set.");
        return;
    }

    with_r(|| {
        let topics = get_help_topics().expect("help indexes should be readable");
        assert!(
            topics.iter().any(|topic| topic.package == "utils"),
            "expected at least one help topic from utils"
        );
        assert!(topics.iter().all(|topic| topic.entry_type == "help"));
    });
}
