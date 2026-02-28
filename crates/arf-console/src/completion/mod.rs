//! Code completion functionality.
//!
//! This module provides R code completion, path completion, and the completion menu UI.

pub mod completer;
pub mod menu;
mod meta;
pub(crate) mod path;
mod r_completer;
mod string_context;
