//! R help system integration.
//!
//! This module provides access to installed package help indexes by reading
//! each package's help metadata files directly.
//!
//! # Acknowledgment
//!
//! This implementation is inspired by the **felp** package by Atsushi Yasumoto (atusy):
//! - Repository: <https://github.com/atusy/felp>
//! - CRAN: <https://cran.r-project.org/package=felp>
//!
//! The concept of searching the installed help database was learned from
//! felp's `fuzzyhelp()` implementation.

mod markdown;
mod r_eval;
mod resolver;
mod target;
mod topics;

pub use markdown::{get_package_help_markdown_by_key_in_dir, rd_source_to_markdown};
pub use r_eval::{eval_r_to_string, get_vignette_text};
pub use resolver::{HelpResolution, HelpTargetResolver};
pub use target::HelpTarget;
pub use topics::get_help_topics_from_paths;

/// A help topic from R's help database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpTopic {
    /// Package name containing this help topic.
    pub package: String,
    /// Installed package directory that supplied this topic's metadata.
    pub package_dir: std::path::PathBuf,
    /// Topic name (the alias used to access the help).
    pub topic: String,
    /// All usable aliases that can be used to find this topic, including the
    /// display alias and any duplicates in the package metadata.
    pub aliases: Vec<String>,
    /// Compiled-help database key from `Meta/Rd.rds`, when available.
    pub help_key: Option<String>,
    /// Title/description of the help topic.
    pub title: String,
    /// Type of help entry (e.g., "help", "vignette", "demo").
    pub entry_type: String,
}

impl HelpTopic {
    /// Format the topic as "package::topic" for display.
    pub fn qualified_name(&self) -> String {
        format!("{}::{}", self.package, self.topic)
    }
}

#[cfg(test)]
fn fixture_path(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/help_metadata")
        .join(name)
}
