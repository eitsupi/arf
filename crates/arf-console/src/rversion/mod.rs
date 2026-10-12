//! Parsing and resolving R version specifications.
//!
//! This module is intentionally independent from the rest of the application
//! so it can be extracted into a separate crate later.

mod files;
mod spec;
#[cfg(test)]
mod tests;

pub use files::{JsonKeyError, TomlKeyError, read_json_key, read_toml_key, read_version_file};
pub use spec::{VersionSpec, VersionSpecParseError, resolve_version};
