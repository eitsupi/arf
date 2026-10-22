//! High-level R abstractions for safe R object manipulation.
//!
//! This crate provides safe Rust wrappers around R's SEXP objects,
//! including automatic protection and type-safe access.

pub mod completion;
mod error;
mod ffi;
pub mod help;
pub mod help_bridge;
pub mod json;
pub mod lib_paths;
mod object;
mod protect;
mod r_string;
mod routines;
pub mod startup;

#[cfg(test)]
mod test_support;

pub use error::*;
pub use object::*;
pub use protect::*;
pub use r_string::escape_r_string;
#[cfg(windows)]
pub use startup::override_platform_gui;
pub use startup::{
    call_dot_first, call_dot_first_sys, should_ignore_site_r_profile, should_ignore_user_r_profile,
    source_site_r_profile, source_user_r_profile,
};
