//! High-level R abstractions for safe R object manipulation.
//!
//! This crate provides safe Rust wrappers around R's SEXP objects,
//! including automatic protection and type-safe access.

pub mod completion;
mod error;
pub mod help;
mod object;
mod protect;
pub mod startup;

pub use error::*;
pub use help::*;
pub use object::*;
pub use protect::*;
pub use startup::{
    should_ignore_site_r_profile, should_ignore_user_r_profile, source_site_r_profile,
    source_user_r_profile,
};
