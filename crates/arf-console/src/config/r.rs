//! R runtime configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

/// R runtime configuration.
///
/// Controls R-specific behavior such as automatic option synchronization.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RConfig {
    /// Automatically sync R's `options(width)` with the terminal width.
    ///
    /// When enabled (default), R's `options(width)` is set to match the terminal
    /// columns at startup and updated dynamically on resize. This ensures output
    /// from functions like `str()`, `print()`, and tibble printing uses the full
    /// available terminal width instead of R's default of 80.
    #[serde(default = "default_true")]
    pub auto_width: bool,
}

impl Default for RConfig {
    fn default() -> Self {
        RConfig { auto_width: true }
    }
}
