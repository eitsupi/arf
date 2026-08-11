//! IPC startup policy configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Settings controlling the syntactic policy applied to IPC `evaluate` calls.
///
/// This is a best-effort syntactic policy, not an R sandbox or a guarantee
/// that an allowed function is non-mutating.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct IpcConfig {
    pub eval: IpcEvalConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct IpcEvalConfig {
    /// Exact R function targets permitted by IPC eval. Operators use their R
    /// spelling (for example `+`, `[`); package targets use `package::function`.
    pub allowed_functions: Vec<String>,
}
