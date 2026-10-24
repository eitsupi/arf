use super::*;
use crate::app::r_source_selection::RSourceSelection;
use crate::app::setup::{RSourceOverrideState, RSourceResolutionReport, RSourceSelectionRoute};
use crate::config::{ConfigLoadProvenance, HistoryLocationSource};
use crate::history::HistoryRuntime;
use crate::ipc::session::{SessionInfo, SessionType};
use info::{HeadlessHistoryRuntime, HeadlessInfo, HeadlessRSourceOverride};

fn report(state: RSourceOverrideState) -> RSourceResolutionReport {
    RSourceResolutionReport {
        status: crate::config::RSourceStatus::Path,
        r_home: Some("/tmp/r-home".into()),
        selection_route: if state == RSourceOverrideState::Applied {
            RSourceSelectionRoute::Override
        } else {
            RSourceSelectionRoute::Startup
        },
        override_provider: None,
        file: None,
        key: None,
        requested_version: None,
        resolved_version: None,
        diagnostics: Vec::new(),
        override_state: state,
        explicit_request: None,
    }
}

#[test]
fn r_source_override_json_object_is_always_present_for_each_state() {
    for state in [
        RSourceOverrideState::Applied,
        RSourceOverrideState::NotConfigured,
        RSourceOverrideState::NoMatch,
        RSourceOverrideState::Failed,
        RSourceOverrideState::Disabled,
        RSourceOverrideState::ShadowedByCli,
    ] {
        let value =
            serde_json::to_value(HeadlessRSourceOverride::from_report(&report(state))).unwrap();
        assert_eq!(value, serde_json::json!({"state": state.as_str()}));
    }
}

#[test]
fn headless_json_includes_r_home_from_session() {
    let history_runtime = HistoryRuntime::initialize(
        &crate::config::HistoryMode::Volatile,
        None,
        crate::history::artifact::HistoryKind::R,
        None,
        None,
    );
    let mut session = SessionInfo {
        pid: 12345,
        socket_path: "/tmp/arf.sock".to_string(),
        r_version: Some("4.4.1".to_string()),
        r_home: Some("/opt/R/4.4.1/lib/R".to_string()),
        cwd: "/tmp".to_string(),
        started_at: "2026-01-01T00:00:00+00:00".to_string(),
        session_type: SessionType::Headless,
        log_file: None,
        history_session_id: None,
    };
    let config_provenance = ConfigLoadProvenance {
        path: "arf.toml".into(),
        startup_r_source_present: true,
        history_migration_warning: None,
    };

    let output = HeadlessInfo::from_session(
        &session,
        &history_runtime,
        HistoryLocationSource::Explicit,
        Vec::new(),
        &report(RSourceOverrideState::NotConfigured),
        std::path::Path::new("/launch"),
        Some(&config_provenance),
    );
    let json = serde_json::to_value(output).unwrap();
    assert_eq!(json["r_home"], "/opt/R/4.4.1/lib/R");
    assert_eq!(json["cwd"], "/tmp");
    assert_eq!(json["r_source_selection"]["selected_by"]["kind"], "default");
    assert_eq!(
        json["r_source_selection"]["metadata"],
        serde_json::Value::Null
    );
    assert_eq!(
        json["r_source_selection"]["selected_by"]["source"]["path"],
        std::path::Path::new("/launch")
            .join("arf.toml")
            .display()
            .to_string()
    );
    assert_eq!(json["history_runtime"]["state"], "volatile");
    assert_eq!(json["history_runtime"]["reason"], "configured");

    session.r_home = None;
    let output = HeadlessInfo::from_session(
        &session,
        &history_runtime,
        HistoryLocationSource::Explicit,
        Vec::new(),
        &report(RSourceOverrideState::NotConfigured),
        std::path::Path::new("/tmp"),
        None,
    );
    let json = serde_json::to_value(output).unwrap();
    assert!(json["r_home"].is_null());
}

#[test]
fn headless_history_runtime_uses_stable_reason_and_separate_path() {
    use crate::history::{
        HistoryFailureDetail, HistoryHandle, HistorySaveReceipt, HistoryStore,
        VolatileHistoryReason,
    };

    let store = HistoryStore::in_memory(None, None).unwrap();
    let handle = || HistoryHandle {
        store: store.clone(),
        receipt: HistorySaveReceipt::new(),
    };
    let configured = HistoryRuntime::Volatile {
        handle: handle(),
        reason: VolatileHistoryReason::Configured,
    };
    let fallback = HistoryRuntime::Volatile {
        handle: handle(),
        reason: VolatileHistoryReason::Fallback {
            persistent_failure: HistoryFailureDetail::test_persistent_open(
                "/tmp/history.db".into(),
            ),
        },
    };
    let fallback_without_path = HistoryRuntime::Volatile {
        handle: handle(),
        reason: VolatileHistoryReason::Fallback {
            persistent_failure: HistoryFailureDetail::test_path_resolution(),
        },
    };
    let unavailable = HistoryRuntime::Unavailable {
        failure: HistoryFailureDetail::test_memory(),
        previous_failure: Some(HistoryFailureDetail::test_persistent_open(
            "/tmp/history.db".into(),
        )),
    };
    let persistent_dir = tempfile::tempdir().unwrap();
    let persistent_path = persistent_dir.path().join("history.db");
    let persistent = HistoryRuntime::Persistent(HistoryHandle {
        store: HistoryStore::open(
            persistent_path.clone(),
            crate::history::artifact::HistoryKind::R,
            None,
            None,
        )
        .unwrap(),
        receipt: HistorySaveReceipt::new(),
    });

    assert!(configured.startup_warning().is_none());
    assert!(fallback.startup_warning().is_some());
    assert!(unavailable.startup_warning().is_some());
    let persistent_json = serde_json::to_value(HeadlessHistoryRuntime::from_runtime(
        &persistent,
        HistoryLocationSource::LegacyDefault,
    ))
    .unwrap();
    assert_eq!(persistent_json["state"], "persistent");
    assert_eq!(persistent_json["source"], "legacy_default");
    assert!(persistent_json["reason"].is_null());
    assert_eq!(
        persistent_json["path"],
        persistent_path.display().to_string()
    );

    let configured_json = serde_json::to_value(HeadlessHistoryRuntime::from_runtime(
        &configured,
        HistoryLocationSource::Volatile,
    ))
    .unwrap();
    assert_eq!(configured_json["state"], "volatile");
    assert_eq!(configured_json["source"], "volatile");
    assert_eq!(configured_json["reason"], "configured");
    assert!(configured_json["path"].is_null());
    assert!(configured_json["detail"].is_null());

    let fallback_json = serde_json::to_value(HeadlessHistoryRuntime::from_runtime(
        &fallback,
        HistoryLocationSource::Default,
    ))
    .unwrap();
    assert_eq!(fallback_json["reason"], "fallback");
    assert_eq!(fallback_json["path"], "/tmp/history.db");
    assert!(!fallback_json["detail"].as_str().unwrap().is_empty());
    let no_path_json = serde_json::to_value(HeadlessHistoryRuntime::from_runtime(
        &fallback_without_path,
        HistoryLocationSource::Default,
    ))
    .unwrap();
    assert_eq!(no_path_json["reason"], "fallback");
    assert!(no_path_json["path"].is_null());

    let unavailable_json = serde_json::to_value(HeadlessHistoryRuntime::from_runtime(
        &unavailable,
        HistoryLocationSource::Default,
    ))
    .unwrap();
    assert_eq!(unavailable_json["state"], "unavailable");
    assert_eq!(unavailable_json["reason"], "initialization_failed");
    assert_eq!(unavailable_json["path"], "/tmp/history.db");
    assert!(
        unavailable_json["detail"]
            .as_str()
            .unwrap()
            .contains("memory initialization failure")
    );
}

#[test]
fn pixi_json_keeps_provider_specific_fields_inside_metadata() {
    let mut report = report(RSourceOverrideState::Applied);
    report.override_provider = Some("pixi".to_owned());
    report.status = config::RSourceStatus::Pixi {
        info: config::PixiRSourceInfo {
            manifest: "pixi.toml".into(),
            environment: "analysis".to_owned(),
            prefix: "prefix".into(),
            target: crate::config::VerifiedRTarget {
                binary: "R.exe".into(),
                home: "home".into(),
                library: "R.dll".into(),
            },
            version: None,
        },
    };

    let value = serde_json::to_value(HeadlessRSourceOverride::from_report(&report)).unwrap();
    assert_eq!(value, serde_json::json!({"state": "applied"}));

    let selection = RSourceSelection::from_report(std::path::Path::new("/"), &report, None);
    let selection_json = serde_json::to_value(selection).unwrap();
    insta::assert_snapshot!(
        serde_json::to_string_pretty(&selection_json).unwrap(),
        @r###"
        {
          "metadata": {
            "environment": "analysis",
            "kind": "pixi",
            "manifest": "/pixi.toml",
            "prefix": "/prefix",
            "r_binary": "/R.exe"
          },
          "provider": "pixi",
          "selected_by": {
            "kind": "provider_request",
            "requested_r_home": null,
            "requested_version": null,
            "source": {
              "format": "toml",
              "key": "experimental.r_source_overrides",
              "kind": "configuration_file",
              "name": null,
              "path": null
            }
          }
        }"###
    );
    let applied_metadata = selection_json["metadata"].clone();

    report.override_state = RSourceOverrideState::ShadowedByCli;
    report.selection_route = RSourceSelectionRoute::Explicit;
    report.explicit_request = Some(crate::app::explicit_r_source::ExplicitRSourceRequest {
        source: crate::app::explicit_r_source::ExplicitRSource::Provider(
            crate::app::explicit_r_source::RProvider::Pixi,
        ),
        origin: crate::app::explicit_r_source::RSourceOrigin::Cli,
    });
    let selection = RSourceSelection::from_report(std::path::Path::new("/"), &report, None);
    let selection_json = serde_json::to_value(selection).unwrap();
    assert_eq!(selection_json["provider"], "pixi");
    assert_eq!(selection_json["selected_by"]["kind"], "provider_request");
    assert_eq!(selection_json["metadata"], applied_metadata);
}
