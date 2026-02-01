//! Editor and input related functionality.
//!
//! This module provides custom edit modes for reedline, prompt formatting,
//! keyboard shortcuts, input validation, editor state tracking, and hinting.

pub mod hinter;
pub mod keybindings;
pub mod mode;
pub mod prompt;
pub mod validator;
pub mod word_nav;

pub use prompt::{PromptFormatter, ViMode};
