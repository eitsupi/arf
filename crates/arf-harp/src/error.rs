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
}

/// Result type for harp operations.
pub type HarpResult<T> = Result<T, HarpError>;
