//! History types for SQLite-backed command history.
//!
//! This module provides types for arf-console history management.
//! Currently, reedline's `HistoryItemExtraInfo` trait is not publicly exported,
//! so we cannot store custom metadata in the `more_info` field.
//! Instead, we rely on the built-in fields: cwd, exit_status, duration, etc.

/// Mode in which a command was executed.
///
/// Note: This is currently used for internal tracking only.
/// Future versions of reedline may allow storing this in history.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryMode {
    /// Normal R command.
    R,
    /// Shell mode command.
    Shell,
    /// Reprex mode command.
    Reprex,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_history_mode_variants() {
        let r = HistoryMode::R;
        let shell = HistoryMode::Shell;
        let reprex = HistoryMode::Reprex;

        assert_ne!(r, shell);
        assert_ne!(shell, reprex);
        assert_ne!(r, reprex);
    }

    #[test]
    fn test_history_mode_clone() {
        let mode = HistoryMode::Shell;
        let cloned = mode;
        assert_eq!(mode, cloned);
    }
}
