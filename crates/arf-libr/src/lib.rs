//! Low-level R FFI bindings using dynamic loading.
//!
//! This crate provides dynamic bindings to R's C API using `libloading`.
//! It allows loading R at runtime without compile-time linking.

mod error;
mod functions;
mod types;

mod sys;

pub use error::*;
pub use functions::*;
pub use sys::*;
pub use types::*;
