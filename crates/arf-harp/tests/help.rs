//! Integration tests for R help rendering via rd2qmd.

// All tests in this file are Linux-only. Gate the entire module to avoid
// unused-import warnings on other platforms (e.g. Windows clippy -D warnings).
#![cfg(target_os = "linux")]

mod common;

use arf_harp::help::{
    HelpResolution, HelpTarget, HelpTargetResolver, get_help_topics_from_paths,
    get_package_help_markdown_by_key_in_dir,
};
use arf_harp::help_bridge::PreparedHelpPage;
use arf_harp::lib_paths::lib_paths;
use common::with_r;
use std::path::PathBuf;

/// Resolve a package-qualified help target against the installed libraries.
///
/// Qualified targets do not depend on the current page, so a placeholder is
/// supplied for it.
fn resolve_qualified(package: &str, topic: &str) -> arf_harp::HarpResult<HelpResolution> {
    let paths = lib_paths().expect(".libPaths() should evaluate");
    let current = PreparedHelpPage {
        package_dir: PathBuf::new(),
        package: String::new(),
        display_topic: String::new(),
        help_key: String::new(),
        markdown: String::new(),
    };
    let target = HelpTarget::from_uri(&format!("x-r-help:{package}/{topic}"))
        .expect("qualified help URI should parse");
    HelpTargetResolver::new(paths).resolve(&current, &target)
}

fn resolve_page(package: &str, topic: &str) -> PreparedHelpPage {
    match resolve_qualified(package, topic) {
        Ok(HelpResolution::Page { page, .. }) => page,
        other => panic!("expected a help page for {package}::{topic}, got: {other:?}"),
    }
}

/// Regression test for GitHub issue #194:
/// `base::solve` contains `%*%` in its Rd source. Without `deparse = TRUE`,
/// `as.character()` emits unescaped `%` which rd-parser treats as a comment,
/// losing closing braces and producing a parse error. With `deparse = TRUE`
/// the `%` is escaped as `\%` and rd2qmd parses the page correctly.
#[test]
fn test_help_base_solve_returns_content() {
    with_r!(test_help_base_solve_returns_content, {
        let md = resolve_page("base", "solve").markdown;
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
        assert!(md.contains("x-r-help:"), "missing help links: {md}");
    });
}

#[test]
fn test_help_package_alias_and_exact_key() {
    with_r!(test_help_package_alias_and_exact_key, {
        let alias = resolve_page("base", "[.data.frame");
        let exact_key = resolve_page("base", "Extract.data.frame");

        assert!(!alias.markdown.is_empty(), "help for alias was empty");
        assert_eq!(alias.help_key, exact_key.help_key);
        assert_eq!(alias.markdown, exact_key.markdown);
    });
}

#[test]
fn test_help_package_embed_fonts_does_not_have_excessive_prose_spaces() {
    with_r!(
        test_help_package_embed_fonts_does_not_have_excessive_prose_spaces,
        {
            let markdown = resolve_page("grDevices", "embedFonts").markdown;
            assert!(
                markdown.contains("and embed all fonts"),
                "unexpected markdown:\n{markdown}"
            );
            assert!(markdown.contains("as the name of a Ghostscript"));
            assert!(!markdown.contains("and   embed"));
            assert!(!markdown.contains("as     the"));
        }
    );
}

#[test]
fn test_help_package_and_topic_not_found() {
    with_r!(test_help_package_and_topic_not_found, {
        assert_eq!(
            resolve_qualified("definitely.not.an.installed.package", "solve")
                .expect("unknown package should resolve cleanly"),
            HelpResolution::NotFound
        );
        assert_eq!(
            resolve_qualified("base", "definitely_not_a_help_topic")
                .expect("unknown topic should resolve cleanly"),
            HelpResolution::NotFound
        );

        let base_dir = resolve_page("base", "solve").package_dir;
        let topic_error = get_package_help_markdown_by_key_in_dir(
            &base_dir,
            "definitely_not_a_help_topic",
            "definitely_not_a_help_topic",
            "base",
        )
        .expect_err("unknown key should fail");
        let message = topic_error.to_string();
        assert!(message.contains("definitely_not_a_help_topic"));
        assert!(message.contains("base"));
    });
}

#[test]
fn test_help_topics_reads_installed_indexes() {
    with_r!(test_help_topics_reads_installed_indexes, {
        let paths = lib_paths().expect(".libPaths() should evaluate");
        let topics = get_help_topics_from_paths(&paths);
        assert!(
            topics.iter().any(|topic| topic.package == "utils"),
            "expected at least one help topic from utils"
        );
        assert!(
            topics
                .iter()
                .filter(|topic| topic.package == "utils")
                .any(|topic| topic.entry_type == "help"),
            "expected at least one help entry from utils"
        );
        assert!(
            topics.iter().any(|topic| {
                topic.package == "glue"
                    && topic.entry_type == "vignette"
                    && !topic.topic.is_empty()
                    && !topic.title.is_empty()
            }),
            "expected a titled vignette topic from glue"
        );
        assert!(
            topics.iter().any(|topic| {
                topic.package == "stats"
                    && topic.entry_type == "demo"
                    && !topic.topic.is_empty()
                    && !topic.title.is_empty()
            }),
            "expected a titled demo topic from stats"
        );
    });
}
