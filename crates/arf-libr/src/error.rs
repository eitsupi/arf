//! Error types for libr.

use thiserror::Error;

/// Errors that can occur when working with R.
#[derive(Error, Debug)]
pub enum RError {
    /// Failed to load the R library.
    #[error("Failed to load R library: {0}")]
    LoadError(#[from] libloading::Error),

    /// R library not found.
    #[error("R library not found at: {0}")]
    LibraryNotFound(String),

    /// R is not initialized.
    #[error("R is not initialized")]
    NotInitialized,

    /// R function not found.
    #[error("R function not found: {0}")]
    FunctionNotFound(String),

    /// R evaluation error.
    #[error("R evaluation error: {0}")]
    EvalError(String),

    /// R parse error.
    #[error("R parse error: {0}")]
    ParseError(String),
}

/// Result type for libr operations.
pub type RResult<T> = Result<T, RError>;
