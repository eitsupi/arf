//! High-level R abstractions for safe R object manipulation.
//!
//! This crate provides safe Rust wrappers around R's SEXP objects,
//! including automatic protection and type-safe access.

pub mod completion;
mod error;
pub mod help;
mod object;
mod protect;

pub use error::*;
pub use help::*;
pub use object::*;
pub use protect::*;
