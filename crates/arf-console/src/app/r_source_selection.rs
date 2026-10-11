//! Shared machine-readable provenance for the selected R source.

use crate::app::explicit_r_source::{ExplicitRSource, RSourceOrigin};
use crate::app::setup::{RSourceResolutionReport, RSourceSelectionRoute};
use crate::config::{ConfigLoadProvenance, RSourceStatus};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Provenance kind is open; clients must accept unknown kinds and fields.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SelectionSource {
    kind: &'static str,
    name: Option<String>,
    path: Option<String>,
    format: Option<&'static str>,
    key: Option<String>,
}

/// Selection condition kind is open; clients must accept unknown kinds/fields.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct SelectedBy {
    kind: &'static str,
    requested_r_home: Option<String>,
    requested_version: Option<String>,
    source: SelectionSource,
}

/// Provider metadata is a tagged open object; clients must accept unknown
/// kinds and fields.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum RSourceMetadata {
    Pixi {
        environment: String,
        manifest: String,
        prefix: String,
        r_binary: String,
    },
    #[serde(rename = "rig-project")]
    RigProject {
        manifest: String,
        r_installation: String,
        r_binary: String,
    },
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct RSourceSelection {
    pub(crate) provider: Option<String>,
    pub(crate) selected_by: SelectedBy,
    pub(crate) metadata: Option<RSourceMetadata>,
}

impl RSourceMetadata {
    pub(crate) fn from_report(report: &RSourceResolutionReport, cwd: &Path) -> Option<Self> {
        match &report.status {
            RSourceStatus::Pixi { info } => Some(Self::Pixi {
                environment: info.environment.clone(),
                manifest: display_path(cwd, &info.manifest),
                prefix: display_path(cwd, &info.prefix),
                r_binary: display_path(cwd, &info.binary),
            }),
            RSourceStatus::RigProject { info } => Some(Self::RigProject {
                manifest: display_path(cwd, &info.manifest),
                r_installation: info.installation_name.clone(),
                r_binary: display_path(cwd, &info.binary),
            }),
            _ => None,
        }
    }
}

impl RSourceSelection {
    pub(crate) fn from_report(
        cwd: &Path,
        report: &RSourceResolutionReport,
        loaded_config: Option<&ConfigLoadProvenance>,
    ) -> Self {
        Self {
            provider: Some(provider(&report.status)),
            selected_by: selected_by(cwd, report, loaded_config),
            metadata: RSourceMetadata::from_report(report, cwd),
        }
    }
}

fn selected_by(
    cwd: &Path,
    report: &RSourceResolutionReport,
    loaded_config: Option<&ConfigLoadProvenance>,
) -> SelectedBy {
    let selection_source = |kind, name, path, format, key| SelectionSource {
        kind,
        name,
        path,
        format,
        key,
    };
    if report.selection_route == RSourceSelectionRoute::Explicit
        && let Some(request) = &report.explicit_request
    {
        let (kind, requested_r_home, requested_version, cli_name, env_name) = match &request.source
        {
            ExplicitRSource::Home(home) => (
                "r_home",
                Some(display_path(cwd, home)),
                None,
                "--r-home",
                "ARF_R_HOME",
            ),
            ExplicitRSource::Version(version) => (
                "version_request",
                None,
                Some(version.clone()),
                "--with-r-version",
                "ARF_R_VERSION",
            ),
            ExplicitRSource::Provider(_) => (
                "provider_request",
                None,
                None,
                "--r-provider",
                "ARF_R_PROVIDER",
            ),
        };
        let (source_kind, source_name) = match request.origin {
            RSourceOrigin::Cli => ("command_line_argument", cli_name),
            RSourceOrigin::Environment => ("environment_variable", env_name),
        };
        return SelectedBy {
            kind,
            requested_r_home,
            requested_version,
            source: selection_source(source_kind, Some(source_name.to_owned()), None, None, None),
        };
    }

    if report.selection_route == RSourceSelectionRoute::Override {
        if matches!(
            report.status,
            RSourceStatus::Pixi { .. } | RSourceStatus::RigProject { .. }
        ) {
            // The provider's identity is reported in `metadata`; the source
            // records where the provider request was configured.
            return SelectedBy {
                kind: "provider_request",
                requested_r_home: None,
                requested_version: None,
                source: selection_source(
                    "configuration_file",
                    None,
                    loaded_config.map(|config| display_path(cwd, &config.path)),
                    Some("toml"),
                    Some("experimental.r_source_overrides".to_owned()),
                ),
            };
        }
        let format = match report.override_provider.as_deref() {
            Some("version-file") => Some("text"),
            Some("toml-key") => Some("toml"),
            Some("json-key") => Some("json"),
            _ => None,
        };
        return SelectedBy {
            kind: "version_request",
            requested_r_home: None,
            requested_version: report.requested_version.clone(),
            source: selection_source(
                "project_file",
                None,
                report.file.as_ref().map(|path| display_path(cwd, path)),
                format,
                report.key.clone(),
            ),
        };
    }

    let source = if let Some(loaded_config) = loaded_config
        && loaded_config.startup_r_source_present
    {
        selection_source(
            "configuration_file",
            None,
            Some(display_path(cwd, &loaded_config.path)),
            Some("toml"),
            Some("startup.r_source".to_owned()),
        )
    } else {
        selection_source("built_in_default", None, None, None, None)
    };
    SelectedBy {
        kind: "default",
        requested_r_home: None,
        requested_version: None,
        source,
    }
}

fn display_path(cwd: &Path, path: &Path) -> String {
    normalize_path(cwd, path).display().to_string()
}

fn normalize_path(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    }
}

fn provider(status: &RSourceStatus) -> String {
    match status {
        RSourceStatus::Rig { .. } => "rig".to_owned(),
        RSourceStatus::Path => "path".to_owned(),
        RSourceStatus::ExplicitPath { .. } => "explicit_path".to_owned(),
        RSourceStatus::Pixi { .. } => "pixi".to_owned(),
        RSourceStatus::RigProject { .. } => "rig-project".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rig_project_selection_keeps_project_metadata_and_provenance_separate() {
        let info = crate::config::RigProjectRSourceInfo {
            root: "/workspace".into(),
            manifest: "/workspace/rproj.toml".into(),
            binary: "/workspace/.rvenv/bin/R".into(),
            home: "/rig/4.6.1/lib/R".into(),
            library: "/rig/4.6.1/lib/R/lib/libR.so".into(),
            installation_name: "4.6.1".into(),
        };
        let report = RSourceResolutionReport {
            status: RSourceStatus::RigProject { info },
            r_home: Some("/rig/4.6.1/lib/R".into()),
            selection_route: RSourceSelectionRoute::Override,
            override_provider: Some("rig-project".into()),
            file: None,
            key: None,
            requested_version: None,
            resolved_version: None,
            diagnostics: Vec::new(),
            override_state: crate::app::setup::RSourceOverrideState::Applied,
            explicit_request: None,
        };

        let loaded_config = ConfigLoadProvenance {
            path: "/config/arf.toml".into(),
            startup_r_source_present: false,
            history_migration_warning: None,
        };
        let value = serde_json::to_value(RSourceSelection::from_report(
            Path::new("/workspace"),
            &report,
            Some(&loaded_config),
        ))
        .unwrap();
        insta::assert_snapshot!(serde_json::to_string_pretty(&value).unwrap(), @r###"
        {
          "metadata": {
            "kind": "rig-project",
            "manifest": "/workspace/rproj.toml",
            "r_binary": "/workspace/.rvenv/bin/R",
            "r_installation": "4.6.1"
          },
          "provider": "rig-project",
          "selected_by": {
            "kind": "provider_request",
            "requested_r_home": null,
            "requested_version": null,
            "source": {
              "format": "toml",
              "key": "experimental.r_source_overrides",
              "kind": "configuration_file",
              "name": null,
              "path": "/config/arf.toml"
            }
          }
        }
        "###);

        let mut explicit_report = report;
        explicit_report.selection_route = RSourceSelectionRoute::Explicit;
        explicit_report.override_provider = None;
        explicit_report.override_state = crate::app::setup::RSourceOverrideState::ShadowedByCli;
        explicit_report.explicit_request =
            Some(crate::app::explicit_r_source::ExplicitRSourceRequest {
                source: ExplicitRSource::Provider(
                    crate::app::explicit_r_source::RProvider::RigProject,
                ),
                origin: crate::app::explicit_r_source::RSourceOrigin::Cli,
            });
        let explicit =
            RSourceSelection::from_report(Path::new("/workspace"), &explicit_report, None);
        assert_eq!(explicit.provider.as_deref(), Some("rig-project"));
        assert_eq!(explicit.selected_by.kind, "provider_request");
        assert_eq!(
            explicit.selected_by.source.name.as_deref(),
            Some("--r-provider")
        );
        assert!(explicit.metadata.is_some());
    }
}
