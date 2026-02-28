//! R code completion for reedline.
//!
//! This module re-exports [`MetaCommandCompleter`] from `meta` and provides
//! [`CombinedCompleter`] which dispatches to the appropriate sub-completer.

use super::r_completer::RCompleter;
use reedline::{Completer, Suggestion};

// Re-export so external code can keep using `crate::completion::completer::MetaCommandCompleter`.
pub use super::meta::MetaCommandCompleter;

/// Combined completer that delegates to the appropriate completer.
pub struct CombinedCompleter {
    r_completer: RCompleter,
    meta_completer: MetaCommandCompleter,
}

impl CombinedCompleter {
    /// Create a new CombinedCompleter with default settings (50ms timeout, 100ms debounce, 50 function check limit).
    pub fn new() -> Self {
        Self::with_settings(50, 100, 50)
    }

    /// Create a new CombinedCompleter with custom settings.
    pub fn with_settings(timeout_ms: u64, debounce_ms: u64, auto_paren_limit: usize) -> Self {
        Self::with_settings_and_rig(timeout_ms, debounce_ms, auto_paren_limit, true)
    }

    /// Create a new CombinedCompleter with custom settings and rig availability.
    ///
    /// When `rig_enabled` is false, the `:switch` command is excluded from completion.
    pub fn with_settings_and_rig(
        timeout_ms: u64,
        debounce_ms: u64,
        auto_paren_limit: usize,
        rig_enabled: bool,
    ) -> Self {
        Self::with_settings_full(
            timeout_ms,
            debounce_ms,
            auto_paren_limit,
            rig_enabled,
            false,
        )
    }

    /// Create a new CombinedCompleter with all settings.
    pub fn with_settings_full(
        timeout_ms: u64,
        debounce_ms: u64,
        auto_paren_limit: usize,
        rig_enabled: bool,
        fuzzy_namespace: bool,
    ) -> Self {
        // Build exclusion list: always exclude `:r` in R mode
        let mut exclusions: Vec<&'static str> = vec!["r"];

        // Exclude `:switch` when rig is not enabled
        if !rig_enabled {
            exclusions.push("switch");
        }

        CombinedCompleter {
            r_completer: RCompleter::with_settings_full(
                timeout_ms,
                debounce_ms,
                auto_paren_limit,
                fuzzy_namespace,
            ),
            meta_completer: MetaCommandCompleter::with_exclusions(exclusions),
        }
    }
}

impl Default for CombinedCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer for CombinedCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        if line.trim_start().starts_with(':') {
            self.meta_completer.complete(line, pos)
        } else {
            self.r_completer.complete(line, pos)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_combined_completer_delegates_to_meta() {
        let mut completer = CombinedCompleter::new();
        let suggestions = completer.complete(":rep", 4);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "reprex");
    }

    #[test]
    fn test_combined_completer_with_leading_whitespace() {
        let mut completer = CombinedCompleter::new();
        let suggestions = completer.complete("  :rep", 6);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].value, "reprex");
    }

    #[test]
    fn test_combined_completer_excludes_r_command() {
        // CombinedCompleter is used in R mode, so `:r` should be excluded
        let mut completer = CombinedCompleter::new();
        let suggestions = completer.complete(":", 1);
        let values: Vec<&str> = suggestions.iter().map(|s| s.value.as_str()).collect();
        assert!(
            !values.contains(&"r"),
            "`:r` should be excluded in CombinedCompleter (R mode)"
        );
        assert!(
            values.contains(&"shell"),
            "`:shell` should be present in R mode"
        );
    }
}
