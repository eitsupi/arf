//! Completion configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Completion configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CompletionConfig {
    /// Enable completion.
    pub enabled: bool,
    /// Completion timeout in milliseconds (max time to wait for R completion).
    pub timeout_ms: u64,
    /// Debounce delay in milliseconds (reuse cached results within this window).
    pub debounce_ms: u64,
    /// Maximum height (rows) for the completion menu.
    pub max_height: u16,
    /// Number of completions to check for function type (for adding parentheses).
    /// Set to 0 to disable function parenthesis insertion.
    /// Only the first N completions are checked to avoid performance issues.
    pub function_paren_check_limit: usize,
}

impl Default for CompletionConfig {
    fn default() -> Self {
        CompletionConfig {
            enabled: true,
            timeout_ms: 50,
            debounce_ms: 100,
            max_height: 10,
            function_paren_check_limit: 50,
        }
    }
}
