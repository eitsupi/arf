//! Error types for harp.

use thiserror::Error;

/// Errors that can occur when working with R objects.
#[derive(Error, Debug)]
pub enum HarpError {
    /// R library error.
    #[error("R library error: {0}")]
    RError(#[from] arf_libr::RError),

    /// Type mismatch.
    #[error("Type mismatch: expected {expected}, got {actual}")]
    TypeMismatch { expected: String, actual: String },

    /// Index out of bounds.
    #[error("Index out of bounds: {index} >= {length}")]
    IndexOutOfBounds { index: usize, length: usize },

    /// Null pointer.
    #[error("Unexpected null pointer")]
    NullPointer,

    /// An installed package could not be found in the cached library paths.
    #[error("Package {package:?} not found in the cached library paths")]
    PackageNotFound { package: String },

    /// An installed package's compiled help database could not be read.
    #[error(
        "Failed to read help for topic {topic:?} in package {package:?} (lookup key {key:?}): {source}"
    )]
    HelpDatabase {
        package: String,
        topic: String,
        key: String,
        #[source]
        source: Box<rd_helpdb::Error>,
    },

    /// A decoded help record could not be lowered to an Rd document.
    #[error(
        "Failed to decode help for topic {topic:?} in package {package:?} (lookup key {key:?}): {source}"
    )]
    HelpLowering {
        package: String,
        topic: String,
        key: String,
        #[source]
        source: Box<rd_ast::LowerError>,
    },
}

/// Result type for harp operations.
pub type HarpResult<T> = Result<T, HarpError>;
