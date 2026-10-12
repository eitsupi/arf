//! Rd to Markdown conversion for terminal help.

use crate::error::{HarpError, HarpResult};
use rd_helpdb::PackageHelpDb;

fn rd_convert_options() -> rd2qmd_core::RdConvertOptions {
    let mut options = rd2qmd_core::RdConvertOptions::default();
    options.code.quarto_code_blocks = false;
    options.arguments_format = rd2qmd_core::ArgumentsFormat::List;
    options.describe_format = rd2qmd_core::DescribeFormat::Headings;
    options.links = super::target::rd_link_options();
    options
}

/// Convert Rd source to terminal help Markdown without evaluating R.
///
/// Cross-references use arf's internal help URI format, interpreted by
/// [`HelpTarget::from_uri`]. Rust consumers of Rd source use the same conversion
/// policy as the R fallback help path.
pub fn rd_source_to_markdown(rd_content: &str) -> HarpResult<String> {
    let parsed = rd_source::parse(rd_content.as_bytes()).map_err(|e| {
        HarpError::RError(arf_libr::RError::EvalError(format!(
            "Failed to parse Rd for Markdown conversion: {}",
            e
        )))
    })?;
    Ok(rd2qmd_core::convert_rd_document(
        parsed.document(),
        &rd_convert_options(),
    ))
}

/// Get package help as Markdown using a known compiled-help key in a specific
/// installed package directory.
///
/// This function never reads `help/aliases.rds`. Callers that already have the
/// package directory avoid selecting another installed copy of the same
/// package.
pub fn get_package_help_markdown_by_key_in_dir(
    package_dir: &std::path::Path,
    display_topic: &str,
    help_key: &str,
    package: &str,
) -> HarpResult<String> {
    let db = open_package_help_db(package_dir, package, display_topic, help_key)?;
    package_help_markdown_from_db(&db, display_topic, help_key, package)
}

pub(super) fn open_package_help_db(
    package_dir: &std::path::Path,
    package: &str,
    display_topic: &str,
    lookup_key: &str,
) -> HarpResult<PackageHelpDb> {
    PackageHelpDb::open(package_dir).map_err(|source| HarpError::HelpDatabase {
        package: package.to_string(),
        topic: display_topic.to_string(),
        key: lookup_key.to_string(),
        source: Box::new(source),
    })
}

pub(super) fn package_help_markdown_from_db(
    db: &PackageHelpDb,
    display_topic: &str,
    lookup_key: &str,
    package: &str,
) -> HarpResult<String> {
    let robj = db
        .raw_topic(lookup_key)
        .map_err(|source| HarpError::HelpDatabase {
            package: package.to_string(),
            topic: display_topic.to_string(),
            key: lookup_key.to_string(),
            source: Box::new(source),
        })?;
    let doc = rd_ast::lower_r_object(&robj).map_err(|source| HarpError::HelpLowering {
        package: package.to_string(),
        topic: display_topic.to_string(),
        key: lookup_key.to_string(),
        source: Box::new(source),
    })?;
    let options = rd_convert_options();
    Ok(rd2qmd_core::convert_rd_document(&doc, &options))
}

#[cfg(test)]
mod tests;
