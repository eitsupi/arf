//! JSON payloads emitted by `arf headless --json`.

use crate::app::r_source_selection::RSourceSelection;
use crate::app::setup::{RSourceOverrideState, RSourceResolutionReport};
use crate::config::{ConfigLoadProvenance, HistoryLocationSource};
use crate::history::HistoryRuntime;
use crate::ipc::policy::IpcPolicy;
use crate::ipc::session::{SessionInfo, SessionType};
use serde::Serialize;

/// JSON output for `arf headless --json`.
///
/// Contains session connection info and any warnings collected during startup.
/// All keys are always present in the JSON output; `r_version`, `r_home`, `log_file`,
/// and `history_session_id` may be `null` only when history initialization is
/// unavailable. `warnings` is an array that may be empty.
#[derive(Debug, Serialize)]
pub(super) struct HeadlessInfo {
    pid: u32,
    socket_path: String,
    r_version: Option<String>,
    r_home: Option<String>,
    cwd: String,
    started_at: String,
    log_file: Option<String>,
    history_session_id: Option<i64>,
    ipc_policy: IpcPolicy,
    history_runtime: HeadlessHistoryRuntime,
    r_source_override: HeadlessRSourceOverride,
    r_source_selection: RSourceSelection,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct HeadlessHistoryRuntime {
    state: String,
    source: &'static str,
    path: Option<String>,
    reason: Option<String>,
    detail: Option<String>,
}

impl HeadlessHistoryRuntime {
    pub(super) fn from_runtime(runtime: &HistoryRuntime, source: HistoryLocationSource) -> Self {
        Self {
            state: runtime.state_name().to_string(),
            source: source.as_str(),
            path: runtime
                .requested_path()
                .map(|path| path.display().to_string()),
            reason: runtime.reason_name().map(str::to_string),
            detail: runtime.diagnostic_detail(),
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct HeadlessRSourceOverride {
    state: String,
}

impl HeadlessRSourceOverride {
    pub(super) fn from_report(report: &RSourceResolutionReport) -> Self {
        let override_state: RSourceOverrideState = report.override_state;
        Self {
            state: override_state.as_str().to_owned(),
        }
    }
}

impl HeadlessInfo {
    pub(super) fn from_session(
        session: &SessionInfo,
        history_runtime: &HistoryRuntime,
        history_source: HistoryLocationSource,
        warnings: Vec<String>,
        resolution: &RSourceResolutionReport,
        launch_cwd: &std::path::Path,
        loaded_config: Option<&ConfigLoadProvenance>,
    ) -> Self {
        // Normalize empty/whitespace-only R version to None so JSON shows null
        let r_version = session
            .r_version
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());

        Self {
            pid: session.pid,
            socket_path: session.socket_path.clone(),
            r_version,
            r_home: session.r_home.clone(),
            cwd: session.cwd.clone(),
            started_at: session.started_at.clone(),
            log_file: session.log_file.clone(),
            history_session_id: session.history_session_id,
            ipc_policy: crate::ipc::policy::policy(SessionType::Headless),
            history_runtime: HeadlessHistoryRuntime::from_runtime(history_runtime, history_source),
            r_source_override: HeadlessRSourceOverride::from_report(resolution),
            r_source_selection: RSourceSelection::from_report(
                launch_cwd,
                resolution,
                loaded_config,
            ),
            warnings,
        }
    }
}
