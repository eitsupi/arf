//! Collection of the session facts shown by the `:info` pager.
//!
//! Everything that reads the environment, the filesystem, external tools or
//! runtime state lives here; turning the result into text is the job of
//! `render`.

use crate::config::{
    ConfigFileInfo, ConfigStatus, RSourceStatus, ReprexFormatter, ReprexMode, mask_home_path,
};
use crate::editor::prompt::get_r_version;
use crate::external::rig;
use crate::history::HistoryRuntime;
use crate::ipc::policy::{IpcPolicy, policy};
use crate::ipc::session::SessionType;
use crate::repl::reprex::ReprexRuntime;
use crate::repl::state::PromptRuntimeConfig;

/// State of the configuration file as seen at collection time.
pub(super) enum ConfigFileState {
    /// No configuration file path is in use.
    Defaults,
    /// A path is configured and the file exists.
    Loaded { path: String, status: ConfigStatus },
    /// A path is configured but the file does not exist.
    NotFound { path: String },
}

/// Result of probing for `rig`.
pub(super) enum RigState {
    /// rig is installed; versions are `(name, is_default)` pairs (possibly empty).
    Installed {
        versions: Vec<(String, bool)>,
    },
    NotInstalled,
    /// rig is installed but the command failed.
    CommandFailed(String),
    /// Any other rig error, already formatted for display.
    Unavailable(String),
}

/// IPC server state and policy.
pub(super) struct IpcInfo {
    pub policy: IpcPolicy,
    pub server_enabled: bool,
}

/// All facts displayed by the session info pager.
pub(super) struct SessionInfo {
    pub config_file: ConfigFileState,
    pub config_source: &'static str,
    /// Empty when R is not available.
    pub r_version: String,
    /// Masked `R_HOME`, if set.
    pub r_home: Option<String>,
    pub r_source: String,
    pub rig: RigState,
    pub shell_mode: bool,
    pub reprex_mode: ReprexMode,
    pub formatter_selector: ReprexFormatter,
    /// Command of the formatter backend that was resolved, if any.
    pub formatter_command: Option<&'static str>,
    pub r_history: String,
    pub shell_history: String,
    /// Set R-related environment variables as `(aligned label, masked value)`.
    pub env_vars: Vec<(&'static str, String)>,
    pub ipc: IpcInfo,
}

impl SessionInfo {
    pub fn collect(
        prompt_config: &PromptRuntimeConfig,
        reprex: &ReprexRuntime,
        config_file_info: &ConfigFileInfo,
        r_history: &HistoryRuntime,
        shell_history: &HistoryRuntime,
        r_source_status: &RSourceStatus,
    ) -> Self {
        Self {
            config_file: config_file_state(config_file_info),
            config_source: config_file_info.source.label(),
            r_version: get_r_version(),
            r_home: std::env::var("R_HOME")
                .ok()
                .map(|r_home| mask_home_path(std::path::Path::new(&r_home))),
            r_source: r_source_status.display(),
            rig: rig_state(),
            shell_mode: prompt_config.is_shell_enabled(),
            reprex_mode: reprex.mode,
            formatter_selector: reprex.formatter_selector,
            formatter_command: reprex.formatter.map(|backend| backend.command()),
            r_history: history_runtime_label(r_history),
            shell_history: history_runtime_label(shell_history),
            env_vars: collect_env_vars(),
            ipc: IpcInfo {
                policy: policy(SessionType::Interactive),
                server_enabled: crate::ipc::server::is_server_running(),
            },
        }
    }
}

fn config_file_state(config_file_info: &ConfigFileInfo) -> ConfigFileState {
    match &config_file_info.path {
        Some(path) if path.exists() => ConfigFileState::Loaded {
            path: mask_home_path(path),
            status: config_file_info.status,
        },
        Some(path) => ConfigFileState::NotFound {
            path: mask_home_path(path),
        },
        None => ConfigFileState::Defaults,
    }
}

fn rig_state() -> RigState {
    match rig::rig_available() {
        Ok(()) => {
            let versions = match rig::list_versions() {
                Ok(versions) => versions
                    .into_iter()
                    .map(|version| (version.name, version.default))
                    .collect(),
                Err(_) => Vec::new(),
            };
            RigState::Installed { versions }
        }
        Err(rig::RigError::NotInstalled) => RigState::NotInstalled,
        Err(rig::RigError::CommandFailed(reason)) => RigState::CommandFailed(reason),
        Err(error) => RigState::Unavailable(error.to_string()),
    }
}

/// R-related environment variables, in display order.
///
/// Each label is padded so that the values line up with the other rows.
const ENV_VARS: [(&str, &str); 6] = [
    ("ARF_HISTORY_DIR", "ARF_HISTORY_DIR: "),
    ("R_LIBS", "R_LIBS:          "),
    ("R_LIBS_USER", "R_LIBS_USER:     "),
    ("R_LIBS_SITE", "R_LIBS_SITE:     "),
    ("R_PROFILE", "R_PROFILE:       "),
    ("R_ENVIRON", "R_ENVIRON:       "),
];

fn collect_env_vars() -> Vec<(&'static str, String)> {
    ENV_VARS
        .iter()
        .filter_map(|(var, label)| {
            // Mask paths in environment variables too
            std::env::var(var)
                .ok()
                .map(|value| (*label, mask_env_value(&value)))
        })
        .collect()
}

pub(super) fn history_runtime_label(runtime: &HistoryRuntime) -> String {
    let label = match runtime {
        HistoryRuntime::Persistent(_) => runtime
            .requested_path()
            .map(|path| format!("persistent ({})", mask_home_path(path)))
            .unwrap_or_else(|| "persistent".to_string()),
        HistoryRuntime::Volatile { reason, .. } => match reason {
            crate::history::VolatileHistoryReason::Configured => {
                "volatile (session only)".to_string()
            }
            crate::history::VolatileHistoryReason::Fallback { .. } => {
                match runtime.requested_path() {
                    Some(path) => format!(
                        "volatile (fallback; requested path: {})",
                        mask_home_path(path)
                    ),
                    None => "volatile (fallback; no persistent path)".to_string(),
                }
            }
        },
        HistoryRuntime::Unavailable { .. } => match runtime.requested_path() {
            Some(path) => format!("unavailable (requested path: {})", mask_home_path(path)),
            None => "unavailable".to_string(),
        },
    };
    match runtime.diagnostic_detail() {
        Some(detail) if !label.contains(&detail) => format!("{label}; {detail}"),
        _ => label,
    }
}

/// Mask home directory in environment variable value.
///
/// Handles path-like values which may contain multiple paths separated by
/// the platform's path list separator (`:` on Unix, `;` on Windows).
/// Each path segment is individually checked and masked if it starts with
/// the home directory.
pub(super) fn mask_env_value(value: &str) -> String {
    let separator = if cfg!(windows) { ';' } else { ':' };

    let masked_parts: Vec<String> = value
        .split(separator)
        .map(|part| {
            let path = std::path::Path::new(part);
            mask_home_path(path)
        })
        .collect();

    masked_parts.join(&separator.to_string())
}
