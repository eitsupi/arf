//! Custom EditMode wrapper for conditional keybinding behavior.
//!
//! This module provides infrastructure to implement buffer-aware keybindings
//! that can check the current cursor position or buffer content before
//! deciding what action to take.
//!
//! # Problem
//!
//! reedline's `EditMode::parse_event()` only receives the raw key event,
//! not the current buffer content. This prevents conditional keybinding
//! behavior like:
//! - ':' should only trigger completion when at line position 0
//! - Other stateful shortcuts can inspect the tracked buffer and cursor
//!
//! # Solution
//!
//! We use a "shadow tracking" approach where the EditMode wrapper maintains
//! an estimate of cursor position and buffer state by observing returned
//! events. This state supports shortcuts and completion behavior that need
//! context unavailable to `EditMode::parse_event` itself.

mod conditions;
mod state;

pub use conditions::{BufferKnownEmpty, ConditionalEditMode, ConditionalRule, CursorAtBegin};
pub use state::{EditorStateRef, new_editor_state_ref};

#[cfg(test)]
mod tests;
