use super::*;
use crate::help::fixture_path;

fn compiled_help_package() -> (tempfile::TempDir, std::path::PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let package_dir = temp.path().join("fixturepkg");
    let help_dir = package_dir.join("help");
    std::fs::create_dir_all(&help_dir).unwrap();
    for extension in ["rdx", "rdb"] {
        std::fs::copy(
            fixture_path(&format!("help_topics.{extension}")),
            help_dir.join(format!("fixturepkg.{extension}")),
        )
        .unwrap();
    }
    (temp, package_dir)
}

#[test]
fn known_help_key_reads_compiled_topic_without_aliases_file() {
    let (_temp, package_dir) = compiled_help_package();
    assert!(!package_dir.join("help/aliases.rds").exists());

    let markdown = get_package_help_markdown_by_key_in_dir(
        &package_dir,
        "display-only-name",
        "first-topic",
        "fixturepkg",
    )
    .expect("direct known-key lookup should not need aliases.rds");

    assert!(!markdown.is_empty());
}

#[test]
fn known_help_key_missing_from_compiled_help_reports_unknown_topic() {
    let (_temp, package_dir) = compiled_help_package();

    let error = get_package_help_markdown_by_key_in_dir(
        &package_dir,
        "display-only-name",
        "second-topic",
        "fixturepkg",
    )
    .expect_err("a key absent from the compiled help database should fail");

    assert!(matches!(
        error,
        HarpError::HelpDatabase { key, source, .. }
            if key == "second-topic"
                && matches!(*source, rd_helpdb::Error::UnknownTopic { ref topic } if topic == "second-topic")
    ));
}

#[test]
fn test_rd_conversion_strips_if_html_content() {
    // Regression test: `\if{html}{\out{...}}` blocks (e.g. asciicast
    // recordings) must not leak raw HTML into terminal help output.
    // Snapshotting the full output (rather than asserting individual
    // substrings) ensures any leaked tag, attribute, or inner text
    // shows up as a diff.
    let rd_content = r#"
\name{hello}
\title{Hello World}
\description{A simple function.}
\details{
Some details.
\if{html}{\out{<div class="asciicast"><span style="color: red;">colored</span></div>}}
More text after.
}
"#;

    let parsed = rd_source::parse(rd_content.as_bytes()).unwrap();
    let options = rd_convert_options();
    let qmd = rd2qmd_core::convert_rd_document(parsed.document(), &options);

    insta::assert_snapshot!("rd_conversion_strips_if_html_content", qmd);
}

#[test]
fn test_rd_conversion_arguments_preserves_blocks() {
    let rd_content = r#"
\name{arguments}
\title{Arguments}
\arguments{
    \item{value}{
        Description before the list.
        \itemize{
            \item First nested item.
            \item Second nested item.
        }

        A second paragraph after the list.
    }
}
"#;

    let parsed = rd_source::parse(rd_content.as_bytes()).unwrap();
    let options = rd_convert_options();
    let qmd = rd2qmd_core::convert_rd_document(parsed.document(), &options);

    insta::assert_snapshot!("rd_conversion_arguments_preserves_blocks", qmd);
}
