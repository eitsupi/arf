//! Directory-level R source overrides.

use super::r_source::{RSourceDiagnostic, RSourceResolutionReport, warning};
use super::rig::{ResolvedRSource, setup_r_via_selected_rig_version};
use crate::config::{RSourceOverride, RSourceOverrideInfo, RSourceStatus};
use crate::external;
use crate::rversion;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub(super) enum OverrideResolution {
    Applied {
        status: Box<RSourceStatus>,
        r_home: PathBuf,
        info: RSourceOverrideInfo,
        diagnostics: Vec<RSourceDiagnostic>,
    },
    Fallback {
        diagnostics: Vec<RSourceDiagnostic>,
    },
}

/// Try the configured directory-level R source overrides in priority order.
pub(super) fn setup_r_via_overrides(
    overrides: &[RSourceOverride],
    base_dir: Option<&Path>,
) -> Option<OverrideResolution> {
    setup_r_via_overrides_with(
        overrides,
        base_dir,
        external::rig::rig_available,
        external::rig::list_versions,
        setup_r_via_selected_rig_version,
    )
}

fn resolve_override_path(file: &Path, base_dir: &mut Option<PathBuf>) -> std::io::Result<PathBuf> {
    if let Some(base_dir) = base_dir.as_ref() {
        return Ok(base_dir.join(file));
    }

    let current_dir = std::env::current_dir()?;
    let path = current_dir.join(file);
    *base_dir = Some(current_dir);
    Ok(path)
}

fn setup_r_via_overrides_with<FAvailable, FList, FResolve>(
    overrides: &[RSourceOverride],
    base_dir: Option<&Path>,
    rig_available: FAvailable,
    list_versions: FList,
    resolve_selected_rig_version: FResolve,
) -> Option<OverrideResolution>
where
    FAvailable: Fn() -> std::result::Result<(), external::rig::RigError>,
    FList: Fn() -> std::result::Result<Vec<external::rig::RigVersion>, external::rig::RigError>,
    FResolve: Fn(&semver::Version, &[external::rig::RigVersion]) -> Result<ResolvedRSource>,
{
    let mut diagnostics = Vec::new();
    let mut evaluated_provider = false;
    let mut rig_available_cache = None;
    let mut installed_versions_cache = None;
    let mut base_dir = base_dir.map(Path::to_path_buf);

    for source in overrides {
        let provider = override_provider_name(source);
        let version = match source {
            RSourceOverride::Pixi => {
                evaluated_provider = true;
                diagnostics.push(warning(
                    "r_source_override.provider_unsupported",
                    format!(
                        "Warning: R source override provider '{provider}' is not implemented; trying the next R source override."
                    ),
                    None,
                ));
                continue;
            }
            RSourceOverride::VersionFile { file } => {
                if !is_bare_filename(file) {
                    evaluated_provider = true;
                    diagnostics.push(invalid_override_file_warning(provider, file));
                    continue;
                }
                let path = match resolve_override_path(file, &mut base_dir) {
                    Ok(path) => path,
                    Err(error) => {
                        evaluated_provider = true;
                        diagnostics.push(warning(
                            "r_source_override.value_invalid",
                            format!(
                                "Warning: Failed to determine the current directory for R source override file {}: {error}; trying the next R source override.",
                                file.display()
                            ),
                            Some(file.clone()),
                        ));
                        continue;
                    }
                };
                match rversion::read_version_file(&path) {
                    Ok(version) => {
                        evaluated_provider = true;
                        version
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        log::debug!("R source override file {} is not present", file.display());
                        continue;
                    }
                    Err(error) => {
                        evaluated_provider = true;
                        diagnostics.push(warning(
                            "r_source_override.value_invalid",
                            format!(
                                "Warning: Failed to read R version override file {}: {error}; trying the next R source override.",
                                file.display()
                            ),
                            Some(file.clone()),
                        ));
                        continue;
                    }
                }
            }
            RSourceOverride::TomlKey { file, key } => {
                if !is_bare_filename(file) {
                    evaluated_provider = true;
                    diagnostics.push(invalid_override_file_warning(provider, file));
                    continue;
                }
                let path = match resolve_override_path(file, &mut base_dir) {
                    Ok(path) => path,
                    Err(error) => {
                        evaluated_provider = true;
                        diagnostics.push(warning(
                            "r_source_override.value_invalid",
                            format!(
                                "Warning: Failed to determine the current directory for R source override file {}: {error}; trying the next R source override.",
                                file.display()
                            ),
                            Some(file.clone()),
                        ));
                        continue;
                    }
                };
                match rversion::read_toml_key(&path, key) {
                    Ok(version) => {
                        evaluated_provider = true;
                        version
                    }
                    Err(error) if error.is_not_found() => {
                        log::debug!("R source override file {} is not present", file.display());
                        continue;
                    }
                    Err(
                        rversion::TomlKeyError::MissingKey(_)
                        | rversion::TomlKeyError::NotString(_),
                    ) => {
                        evaluated_provider = true;
                        diagnostics.push(warning(
                            "r_source_override.value_invalid",
                            format!(
                                "Warning: {}:{} does not contain the configured R version key; trying the next R source override.",
                                file.display(),
                                key
                            ),
                            Some(file.clone()),
                        ));
                        continue;
                    }
                    Err(rversion::TomlKeyError::Parse(_)) => {
                        evaluated_provider = true;
                        diagnostics.push(warning(
                            "r_source_override.value_invalid",
                            format!(
                                "Warning: Failed to parse R source override file {}; trying the next R source override.",
                                file.display()
                            ),
                            Some(file.clone()),
                        ));
                        continue;
                    }
                    Err(error) => {
                        evaluated_provider = true;
                        diagnostics.push(warning(
                            "r_source_override.value_invalid",
                            format!(
                                "Warning: Failed to read R version from TOML key '{}' in {}: {error}; trying the next R source override.",
                                key,
                                file.display()
                            ),
                            Some(file.clone()),
                        ));
                        continue;
                    }
                }
            }
            RSourceOverride::JsonKey { file, key } => {
                if !is_bare_filename(file) {
                    evaluated_provider = true;
                    diagnostics.push(invalid_override_file_warning(provider, file));
                    continue;
                }
                let path = match resolve_override_path(file, &mut base_dir) {
                    Ok(path) => path,
                    Err(error) => {
                        evaluated_provider = true;
                        diagnostics.push(warning(
                            "r_source_override.value_invalid",
                            format!(
                                "Warning: Failed to determine the current directory for R source override file {}: {error}; trying the next R source override.",
                                file.display()
                            ),
                            Some(file.clone()),
                        ));
                        continue;
                    }
                };
                match rversion::read_json_key(&path, key) {
                    Ok(version) => {
                        evaluated_provider = true;
                        version
                    }
                    Err(error) if error.is_not_found() => {
                        log::debug!("R source override file {} is not present", file.display());
                        continue;
                    }
                    Err(
                        rversion::JsonKeyError::MissingKey(_)
                        | rversion::JsonKeyError::NotString(_),
                    ) => {
                        evaluated_provider = true;
                        diagnostics.push(warning(
                            "r_source_override.value_invalid",
                            format!(
                                "Warning: {}:{} does not contain the configured R version key; trying the next R source override.",
                                file.display(),
                                key
                            ),
                            Some(file.clone()),
                        ));
                        continue;
                    }
                    Err(rversion::JsonKeyError::Parse(_)) => {
                        evaluated_provider = true;
                        diagnostics.push(warning(
                            "r_source_override.value_invalid",
                            format!(
                                "Warning: Failed to parse R source override file {}; trying the next R source override.",
                                file.display()
                            ),
                            Some(file.clone()),
                        ));
                        continue;
                    }
                    Err(error) => {
                        evaluated_provider = true;
                        diagnostics.push(warning(
                            "r_source_override.value_invalid",
                            format!(
                                "Warning: Failed to read R version from JSON key '{}' in {}: {error}; trying the next R source override.",
                                key,
                                file.display()
                            ),
                            Some(file.clone()),
                        ));
                        continue;
                    }
                }
            }
        };

        let trimmed_version = version.trim().to_owned();
        let spec = match rversion::VersionSpec::parse(&trimmed_version) {
            Ok(spec) => spec,
            Err(rversion::VersionSpecParseError::Empty) => {
                diagnostics.push(warning(
                    "r_source_override.value_invalid",
                    format!(
                        "Warning: {} contains an empty R version specification; trying the next R source override.",
                        override_location(source)
                    ),
                    override_file(source),
                ));
                continue;
            }
            Err(rversion::VersionSpecParseError::Invalid) => {
                diagnostics.push(warning(
                    "r_source_override.value_invalid",
                    format!(
                        "Warning: {} contains an R version specification that could not be parsed; trying the next R source override.",
                        override_location(source)
                    ),
                    override_file(source),
                ));
                continue;
            }
        };

        if let rversion::VersionSpec::Named(name) = &spec {
            diagnostics.push(warning(
                "r_source_override.provider_unsupported",
                format!(
                    "Warning: R version \"{name}\" from {} is unsupported in the R source override path; trying the next R source override.",
                    override_location(source)
                ),
                override_file(source),
            ));
            continue;
        }

        let rig_availability = rig_available_cache.get_or_insert_with(&rig_available);
        if let Err(error) = rig_availability {
            diagnostics.push(rig_unavailable_warning(error));
            return Some(OverrideResolution::Fallback { diagnostics });
        }

        let installed = if let Some(installed) = installed_versions_cache.as_ref() {
            installed
        } else {
            match list_versions() {
                Ok(versions) => installed_versions_cache.insert(versions),
                Err(error) => {
                    diagnostics.push(warning(
                        "r_source_override.fallback",
                        format!(
                            "Warning: Could not inspect installed R versions for {}: {error}; falling back to startup.r_source.",
                            override_location(source)
                        ),
                        override_file(source),
                    ));
                    return Some(OverrideResolution::Fallback { diagnostics });
                }
            }
        };
        let installed_versions = installed
            .iter()
            .filter_map(|installed| semver::Version::parse(&installed.version).ok())
            .collect::<Vec<_>>();

        let Some(selected) = rversion::resolve_version(&spec, &installed_versions) else {
            diagnostics.push(not_installed_warning(
                provider,
                &override_location(source),
                &trimmed_version,
                &spec,
                override_file(source),
            ));
            continue;
        };

        match resolve_selected_rig_version(selected, installed) {
            Ok(ResolvedRSource {
                status: RSourceStatus::Rig { version, .. },
                r_home,
            }) => {
                let info = RSourceOverrideInfo {
                    provider: provider.to_owned(),
                    file: override_file(source),
                    key: override_key(source),
                    requested_version: trimmed_version,
                    resolved_version: version.clone(),
                };
                let Some(r_home) = r_home else {
                    diagnostics.push(warning(
                        "r_source_override.fallback",
                        "Warning: R source override resolved without an R_HOME; falling back to startup.r_source.",
                        override_file(source),
                    ));
                    return Some(OverrideResolution::Fallback { diagnostics });
                };
                return Some(OverrideResolution::Applied {
                    status: RSourceStatus::Rig {
                        version,
                        override_info: Some(info.clone()),
                    }
                    .into(),
                    r_home,
                    info,
                    diagnostics,
                });
            }
            Ok(status) => {
                diagnostics.push(warning(
                    "r_source_override.provider_unsupported",
                    format!(
                        "Warning: R source override {} resolved to an unsupported R source status ({status:?}); falling back to startup.r_source.",
                        override_location(source)
                    ),
                    override_file(source),
                ));
                return Some(OverrideResolution::Fallback { diagnostics });
            }
            Err(error) => {
                diagnostics.push(warning(
                    "r_source_override.resolution_failed",
                    format!(
                        "Warning: Failed to use R version \"{}\" from {}: {error}; trying the next R source override.",
                        trimmed_version,
                        override_location(source)
                    ),
                    override_file(source),
                ));
                continue;
            }
        }
    }

    if evaluated_provider {
        diagnostics.push(fallback_warning());
        Some(OverrideResolution::Fallback { diagnostics })
    } else {
        None
    }
}

fn is_bare_filename(file: &Path) -> bool {
    let Some(file) = file.to_str() else {
        return false;
    };

    if file.is_empty()
        || file == "."
        || file == ".."
        || file.contains('/')
        || file.contains('\\')
        || Path::new(file).is_absolute()
    {
        return false;
    }

    let bytes = file.as_bytes();
    !(bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

fn invalid_override_file_warning(provider: &str, file: &Path) -> RSourceDiagnostic {
    warning(
        "r_source_override.value_invalid",
        format!(
            "Warning: R source override provider '{provider}' has invalid file '{}'; file must be a bare filename; trying the next R source override.",
            file.display()
        ),
        Some(file.to_path_buf()),
    )
}

pub(super) fn script_override_notice(resolution: &RSourceResolutionReport) -> Option<String> {
    resolution
        .override_info()
        .map(|info| format!("# R source override: {}", info.display()))
}

fn override_provider_name(source: &RSourceOverride) -> &'static str {
    match source {
        RSourceOverride::Pixi => "pixi",
        RSourceOverride::VersionFile { .. } => "version-file",
        RSourceOverride::TomlKey { .. } => "toml-key",
        RSourceOverride::JsonKey { .. } => "json-key",
    }
}

fn override_file(source: &RSourceOverride) -> Option<std::path::PathBuf> {
    match source {
        RSourceOverride::Pixi => None,
        RSourceOverride::VersionFile { file }
        | RSourceOverride::TomlKey { file, .. }
        | RSourceOverride::JsonKey { file, .. } => Some(file.clone()),
    }
}

fn override_key(source: &RSourceOverride) -> Option<String> {
    match source {
        RSourceOverride::TomlKey { key, .. } | RSourceOverride::JsonKey { key, .. } => {
            Some(key.clone())
        }
        RSourceOverride::Pixi | RSourceOverride::VersionFile { .. } => None,
    }
}

fn override_location(source: &RSourceOverride) -> String {
    match source {
        RSourceOverride::Pixi => "pixi".to_owned(),
        RSourceOverride::VersionFile { file } => file.display().to_string(),
        RSourceOverride::TomlKey { file, key } => format!("{}:{}", file.display(), key),
        RSourceOverride::JsonKey { file, key } => format!("{}:{}", file.display(), key),
    }
}

fn rig_unavailable_warning(error: &external::rig::RigError) -> RSourceDiagnostic {
    let message = match error {
        external::rig::RigError::NotInstalled => {
            "Warning: rig is not installed, so the R source override cannot be resolved.\n         Install rig from https://github.com/r-lib/rig or use \"auto\".\n         Falling back to startup.r_source."
                .to_string()
        }
        external::rig::RigError::CommandFailed(reason) => format!(
            "Warning: rig is installed but could not be run, so the R source override cannot be resolved: {reason}.\n         Fix the rig installation or command, then restart arf.\n         Falling back to startup.r_source."
        ),
        error => format!(
            "Warning: rig availability could not be checked ({error}), so the R source override cannot be resolved.\n         Fix the rig installation or command, then restart arf.\n         Falling back to startup.r_source."
        ),
    };

    warning("r_source_override.rig_unavailable", message, None)
}

fn fallback_warning() -> RSourceDiagnostic {
    warning(
        "r_source_override.fallback",
        "Warning: All R source overrides failed.\n         Falling back to startup.r_source.",
        None,
    )
}

fn not_installed_warning(
    provider: &str,
    location: &str,
    version: &str,
    spec: &rversion::VersionSpec,
    path: Option<PathBuf>,
) -> RSourceDiagnostic {
    let message = if spec.is_concrete_version() {
        format!(
            "Warning: R source override provider '{provider}' at {location} requested R version \"{version}\", which is not installed.\n         Install it with rig add {version}, then restart arf.\n         Trying the next R source override."
        )
    } else {
        format!(
            "Warning: R source override provider '{provider}' at {location} has no installed R version matching specification \"{version}\".\n         Install a matching R version with rig, then restart arf.\n         Trying the next R source override."
        )
    };
    warning("r_source_override.version_not_installed", message, path)
}

#[cfg(test)]
mod r_source_override_tests {
    use super::super::r_source::{
        RSourceOverrideState, RSourceResolutionReport, apply_r_source,
        resolve_path_r_home_for_report_with, resolve_r_source, setup_r_fallback_with,
    };
    use super::super::rig::ResolvedRSource;
    use super::*;
    use crate::config::{Config, RSource, RSourceMode};
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn diagnostic_text(diagnostics: &[RSourceDiagnostic]) -> String {
        diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn path_source(path: &Path) -> RSource {
        RSource::Path {
            path: path.to_path_buf(),
        }
    }

    fn setup_r(
        r_source: &RSource,
        r_source_overrides: &[RSourceOverride],
        base_dir: Option<&Path>,
        cli_r_home: Option<&Path>,
        cli_version: Option<&str>,
        no_r_source_overrides: bool,
    ) -> Result<RSourceResolutionReport> {
        resolve_r_source(
            r_source,
            r_source_overrides,
            base_dir,
            cli_r_home,
            cli_version,
            no_r_source_overrides,
        )
    }

    #[test]
    fn resolve_r_source_does_not_mutate_r_home() {
        // Hold the lock to keep concurrent environment writers out while reading R_HOME.
        let _guard = crate::test_utils::lock_env();
        let original = std::env::var_os("R_HOME");
        let temp = tempfile::tempdir().unwrap();

        let report = resolve_r_source(
            &path_source(temp.path()),
            &[],
            None,
            Some(temp.path()),
            None,
            false,
        )
        .unwrap();

        assert_eq!(report.r_home.as_deref(), Some(temp.path()));
        assert_eq!(std::env::var_os("R_HOME"), original);
    }

    #[test]
    fn path_resolution_leaves_r_home_for_r_initialization() {
        let report = RSourceResolutionReport::from_status(
            RSourceStatus::Path,
            None,
            RSourceOverrideState::Disabled,
        );
        assert!(matches!(report.status, RSourceStatus::Path));
        assert!(report.r_home.is_none());
    }

    #[test]
    fn path_fallback_without_rig_reports_path_without_r_home() {
        let report = setup_r_fallback_with(
            &RSource::Mode(RSourceMode::Auto),
            RSourceOverrideState::NotConfigured,
            || Err(external::rig::RigError::NotInstalled),
            |_| panic!("rig's default version should not be resolved"),
        )
        .unwrap();

        assert!(matches!(report.status, RSourceStatus::Path));
        assert!(report.r_home.is_none());
    }

    #[test]
    fn path_fallback_when_rig_default_resolution_fails_has_no_r_home() {
        let report = setup_r_fallback_with(
            &RSource::Mode(RSourceMode::Auto),
            RSourceOverrideState::NotConfigured,
            || Ok(()),
            |_| Err(external::rig::RigError::NoDefaultVersion),
        )
        .unwrap();

        assert!(matches!(report.status, RSourceStatus::Path));
        assert!(report.r_home.is_none());
    }

    #[test]
    fn path_r_home_discovery_failure_is_reported_deterministically() {
        let mut report = RSourceResolutionReport::from_status(
            RSourceStatus::Path,
            None,
            RSourceOverrideState::Disabled,
        );

        resolve_path_r_home_for_report_with(&mut report, || {
            Err(arf_libr::RError::LibraryNotFound(
                "injected discovery failure".to_string(),
            ))
        });

        assert!(report.r_home.is_none());
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].code, "r_discovery.failed");
        assert_eq!(
            report.diagnostics[0].message,
            "Warning: Failed to determine R_HOME from PATH: Failed to discover the R library from PATH"
        );
    }

    #[test]
    fn apply_r_source_does_not_set_r_home_for_path_mode() {
        // Hold the lock to keep concurrent environment writers out while reading R_HOME.
        let _guard = crate::test_utils::lock_env();
        let original = std::env::var_os("R_HOME");
        let report = RSourceResolutionReport {
            status: RSourceStatus::Path,
            r_home: Some(PathBuf::from("/tmp/discovered-r-home")),
            provider: None,
            file: None,
            key: None,
            requested_version: None,
            resolved_version: None,
            diagnostics: Vec::new(),
            override_state: RSourceOverrideState::NotConfigured,
        };

        apply_r_source(&report).unwrap();

        assert_eq!(std::env::var_os("R_HOME"), original);
    }

    #[test]
    fn cli_resolution_has_priority_over_overrides_and_config() {
        let config_path = tempfile::tempdir().unwrap();
        let cli_path = tempfile::tempdir().unwrap();
        let report = resolve_r_source(
            &path_source(config_path.path()),
            &[RSourceOverride::Pixi],
            Some(config_path.path()),
            Some(cli_path.path()),
            None,
            false,
        )
        .unwrap();

        assert_eq!(report.r_home.as_deref(), Some(cli_path.path()));
        assert_eq!(report.override_state, RSourceOverrideState::ShadowedByCli);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn toml_override_does_not_search_parent_directories() {
        let _cwd_lock = crate::test_utils::lock_cwd();
        let parent = tempfile::tempdir().unwrap();
        let child = parent.path().join("child");
        std::fs::create_dir(&child).unwrap();
        std::fs::write(
            parent.path().join("rproject.toml"),
            "[project]\nr_version = \"4.4\"\n",
        )
        .unwrap();
        std::env::set_current_dir(&child).unwrap();

        let result = rversion::read_toml_key(Path::new("rproject.toml"), "project.r_version");

        assert!(matches!(
            result,
            Err(rversion::TomlKeyError::Read(error))
                if error.kind() == std::io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn default_override_resolution_has_no_match_and_no_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let report = setup_r(
            &path_source(temp.path()),
            &[RSourceOverride::VersionFile {
                file: ".r-version".into(),
            }],
            Some(temp.path()),
            None,
            None,
            false,
        )
        .unwrap();

        assert_eq!(report.override_state, RSourceOverrideState::NoMatch);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn empty_override_configuration_is_not_configured() {
        let temp = tempfile::tempdir().unwrap();
        let report = setup_r(&path_source(temp.path()), &[], None, None, None, false).unwrap();

        assert_eq!(report.override_state, RSourceOverrideState::NotConfigured);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn pixi_only_override_resolution_does_not_need_a_base_dir() {
        let temp = tempfile::tempdir().unwrap();
        let report = setup_r(
            &path_source(temp.path()),
            &[RSourceOverride::Pixi],
            None,
            None,
            None,
            false,
        )
        .unwrap();

        assert_eq!(report.override_state, RSourceOverrideState::Failed);
        assert!(report.diagnostics[0].message.contains("pixi"));
    }

    #[test]
    fn disabled_override_resolution_has_no_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let report = setup_r(
            &path_source(temp.path()),
            &[RSourceOverride::Pixi],
            Some(temp.path()),
            None,
            None,
            true,
        )
        .unwrap();

        assert_eq!(report.override_state, RSourceOverrideState::Disabled);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn cli_r_home_shadows_overrides_without_a_conflict() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("rproject.toml"),
            "[project]\nr_version = \"4.4\"\n",
        )
        .unwrap();
        let report = setup_r(
            &path_source(temp.path()),
            &[RSourceOverride::TomlKey {
                file: "rproject.toml".into(),
                key: "project.r_version".to_string(),
            }],
            Some(temp.path()),
            Some(temp.path()),
            None,
            false,
        )
        .unwrap();

        assert_eq!(report.override_state, RSourceOverrideState::ShadowedByCli);
        assert!(report.diagnostics.is_empty());
        assert!(matches!(report.status, RSourceStatus::ExplicitPath { .. }));
    }

    #[test]
    fn disabled_override_resolution_reports_disabled_for_cli_r_home() {
        let temp = tempfile::tempdir().unwrap();
        let report = setup_r(
            &path_source(temp.path()),
            &[RSourceOverride::Pixi],
            Some(temp.path()),
            Some(temp.path()),
            None,
            true,
        )
        .unwrap();

        assert_eq!(report.override_state, RSourceOverrideState::Disabled);
        assert!(matches!(report.status, RSourceStatus::ExplicitPath { .. }));
    }

    #[test]
    fn unsupported_pixi_provider_is_reported_and_falls_back() {
        let temp = tempfile::tempdir().unwrap();
        let report = setup_r(
            &path_source(temp.path()),
            &[RSourceOverride::Pixi],
            Some(temp.path()),
            None,
            None,
            false,
        )
        .unwrap();

        assert_eq!(report.override_state, RSourceOverrideState::Failed);
        assert_eq!(report.diagnostics.len(), 2);
        assert_eq!(
            report.diagnostics[0].code,
            "r_source_override.provider_unsupported"
        );
        assert_eq!(report.diagnostics[1].code, "r_source_override.fallback");
        assert!(report.diagnostics[0].message.contains("pixi"));
        assert!(
            report.diagnostics[1]
                .message
                .contains("Falling back to startup.r_source")
        );
    }

    #[test]
    fn override_status_display_includes_resolution_metadata() {
        let info = RSourceOverrideInfo {
            provider: "toml-key".to_string(),
            file: Some("rproject.toml".into()),
            key: Some("project.r_version".to_string()),
            requested_version: "4.4".to_string(),
            resolved_version: "4.4.2".to_string(),
        };
        let status = RSourceStatus::Rig {
            version: "4.4.2".to_string(),
            override_info: Some(info),
        };

        assert!(status.rig_enabled());
        assert_eq!(
            status.display(),
            "rig (R 4.4.2; override: toml-key rproject.toml:project.r_version = \"4.4\")"
        );
    }

    #[test]
    fn rig_unavailable_warning_explains_fallback() {
        assert_eq!(
            rig_unavailable_warning(&external::rig::RigError::NotInstalled).message,
            "Warning: rig is not installed, so the R source override cannot be resolved.\n         Install rig from https://github.com/r-lib/rig or use \"auto\".\n         Falling back to startup.r_source."
        );
    }

    #[test]
    fn command_failed_rig_warning_preserves_reason_without_install_guidance() {
        let temp = tempfile::tempdir().unwrap();
        let file = Path::new("project.r-version").to_path_buf();
        std::fs::write(temp.path().join(&file), "4.4.2\n").unwrap();

        let result = setup_r_via_overrides_with(
            &[RSourceOverride::VersionFile { file }],
            Some(temp.path()),
            || {
                Err(external::rig::RigError::CommandFailed(
                    "permission denied".to_string(),
                ))
            },
            || panic!("rig versions should not be listed after availability fails"),
            |_, _| panic!("R version should not be resolved after availability fails"),
        )
        .expect("an existing override should produce a fallback");

        let OverrideResolution::Fallback { diagnostics } = result else {
            panic!("a failed rig command should fall back to startup.r_source");
        };
        let message = diagnostic_text(&diagnostics);
        assert!(message.contains("permission denied"));
        assert!(!message.contains("Install rig"));
    }

    #[test]
    fn range_not_installed_warning_does_not_suggest_rig_add() {
        let spec = rversion::VersionSpec::parse(">=4.3, <5.0").unwrap();
        let warning = not_installed_warning(
            "toml-key",
            "rproject.toml:project.r_version",
            ">=4.3, <5.0",
            &spec,
            Some(PathBuf::from("rproject.toml")),
        );

        assert_eq!(warning.code, "r_source_override.version_not_installed");
        assert!(
            warning
                .message
                .contains("Install a matching R version with rig")
        );
        assert!(!warning.message.contains("rig add"));
        assert!(warning.message.contains("toml-key"));
        assert!(warning.message.contains("rproject.toml:project.r_version"));
        assert!(!warning.message.contains("Falling back to startup.r_source"));
    }

    #[test]
    fn script_override_notice_is_available_with_default_banner_setting() {
        let config = Config::default();
        assert!(config.startup.show_banner);

        let info = RSourceOverrideInfo {
            provider: "version-file".to_string(),
            file: Some(".r-version".into()),
            key: None,
            requested_version: "4.4".to_string(),
            resolved_version: "4.4.2".to_string(),
        };
        let report = RSourceResolutionReport::applied(
            RSourceStatus::Rig {
                version: "4.4.2".to_string(),
                override_info: Some(info),
            },
            PathBuf::from("/tmp/r-home"),
            RSourceOverrideInfo {
                provider: "version-file".to_string(),
                file: Some(".r-version".into()),
                key: None,
                requested_version: "4.4".to_string(),
                resolved_version: "4.4.2".to_string(),
            },
            Vec::new(),
        );

        assert_eq!(
            script_override_notice(&report).as_deref(),
            Some("# R source override: version-file .r-version = \"4.4\"")
        );
    }

    #[test]
    fn not_installed_override_falls_through_to_installed_provider() {
        let temp = tempfile::tempdir().unwrap();
        let first_file = Path::new("first.r-version").to_path_buf();
        let second_file = Path::new("second.r-version").to_path_buf();
        std::fs::write(temp.path().join(&first_file), "4.3.0\n").unwrap();
        std::fs::write(temp.path().join(&second_file), "4.4.2\n").unwrap();

        let result = setup_r_via_overrides_with(
            &[
                RSourceOverride::VersionFile { file: first_file },
                RSourceOverride::VersionFile {
                    file: second_file.clone(),
                },
            ],
            Some(temp.path()),
            || Ok(()),
            || Ok(vec![rig_version("4.4.2")]),
            |selected, versions| {
                assert_eq!(selected, &semver::Version::new(4, 4, 2));
                assert_eq!(versions[0].version, "4.4.2");
                Ok(ResolvedRSource {
                    status: RSourceStatus::Rig {
                        version: versions[0].version.clone(),
                        override_info: None,
                    },
                    r_home: Some(PathBuf::from("/tmp/r-home")),
                })
            },
        )
        .unwrap();

        match result {
            OverrideResolution::Applied {
                status,
                info,
                diagnostics,
                ..
            } => {
                assert!(matches!(*status, RSourceStatus::Rig { .. }));
                assert_eq!(info.file, Some(second_file));
                assert_eq!(info.resolved_version, "4.4.2");
                assert_eq!(diagnostics.len(), 1);
                assert_eq!(
                    diagnostics[0].code,
                    "r_source_override.version_not_installed"
                );
                assert!(diagnostics[0].message.contains("4.3.0"));
                assert!(
                    diagnostics[0]
                        .message
                        .contains("Trying the next R source override")
                );
                assert!(
                    !diagnostics[0]
                        .message
                        .contains("Falling back to startup.r_source")
                );
            }
            OverrideResolution::Fallback { .. } => {
                panic!("the installed provider should be applied")
            }
        }
    }

    #[test]
    fn version_file_resolution_uses_first_line_only() {
        let temp = tempfile::tempdir().unwrap();
        let file = Path::new("project.r-version");
        std::fs::write(
            temp.path().join(file),
            "4.4.2\nprivate trailing contents that must not be read\n",
        )
        .unwrap();

        let result = setup_r_via_overrides_with(
            &[RSourceOverride::VersionFile {
                file: file.to_path_buf(),
            }],
            Some(temp.path()),
            || Ok(()),
            || Ok(vec![rig_version("4.4.2")]),
            |selected, versions| {
                assert_eq!(selected, &semver::Version::new(4, 4, 2));
                Ok(ResolvedRSource {
                    status: RSourceStatus::Rig {
                        version: versions[0].version.clone(),
                        override_info: None,
                    },
                    r_home: Some(PathBuf::from("/tmp/r-home")),
                })
            },
        )
        .unwrap();

        match result {
            OverrideResolution::Applied {
                info, diagnostics, ..
            } => {
                assert_eq!(info.requested_version, "4.4.2");
                assert!(diagnostics.is_empty());
            }
            OverrideResolution::Fallback { .. } => {
                panic!("the first version-file line should resolve")
            }
        }
    }

    #[test]
    fn json_key_resolution_uses_nested_string_and_reports_metadata() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("renv.lock"), r#"{"R":{"Version":"4.4"}}"#).unwrap();

        let result = setup_r_via_overrides_with(
            &[RSourceOverride::JsonKey {
                file: "renv.lock".into(),
                key: "R.Version".to_string(),
            }],
            Some(temp.path()),
            || Ok(()),
            || Ok(vec![rig_version("4.4.2")]),
            |selected, versions| {
                assert_eq!(selected, &semver::Version::new(4, 4, 2));
                Ok(ResolvedRSource {
                    status: RSourceStatus::Rig {
                        version: versions[0].version.clone(),
                        override_info: None,
                    },
                    r_home: Some(PathBuf::from("/tmp/r-home")),
                })
            },
        )
        .unwrap();

        match result {
            OverrideResolution::Applied { info, .. } => {
                assert_eq!(info.provider, "json-key");
                assert_eq!(info.file, Some(PathBuf::from("renv.lock")));
                assert_eq!(info.key.as_deref(), Some("R.Version"));
                assert_eq!(info.requested_version, "4.4");
            }
            OverrideResolution::Fallback { .. } => {
                panic!("the JSON key provider should be applied")
            }
        }
    }

    #[test]
    fn json_key_missing_value_falls_through_to_next_provider() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("renv.lock"), r#"{"R":{}}"#).unwrap();
        let second_file = Path::new("fallback.r-version").to_path_buf();
        std::fs::write(temp.path().join(&second_file), "4.4.2\n").unwrap();

        let result = setup_r_via_overrides_with(
            &[
                RSourceOverride::JsonKey {
                    file: "renv.lock".into(),
                    key: "R.Version".to_string(),
                },
                RSourceOverride::VersionFile {
                    file: second_file.clone(),
                },
            ],
            Some(temp.path()),
            || Ok(()),
            || Ok(vec![rig_version("4.4.2")]),
            |selected, versions| {
                assert_eq!(selected, &semver::Version::new(4, 4, 2));
                Ok(ResolvedRSource {
                    status: RSourceStatus::Rig {
                        version: versions[0].version.clone(),
                        override_info: None,
                    },
                    r_home: Some(PathBuf::from("/tmp/r-home")),
                })
            },
        )
        .unwrap();

        match result {
            OverrideResolution::Applied {
                info, diagnostics, ..
            } => {
                assert_eq!(info.provider, "version-file");
                assert_eq!(info.file, Some(second_file));
                assert_eq!(diagnostics.len(), 1);
                assert!(diagnostics[0].message.contains("renv.lock:R.Version"));
            }
            OverrideResolution::Fallback { .. } => {
                panic!("the fallback provider should be applied")
            }
        }
    }

    #[test]
    fn json_key_value_errors_fall_through_to_next_provider() {
        for (name, contents) in [
            ("missing", r#"{"R":{}}"#),
            ("non-string", r#"{"R":{"Version":4.4}}"#),
            ("parse", r#"{"R":{"Version":"4.4"}"#),
        ] {
            let temp = tempfile::tempdir().unwrap();
            std::fs::write(temp.path().join("renv.lock"), contents).unwrap();
            std::fs::write(temp.path().join("fallback.r-version"), "4.4.2\n").unwrap();

            let result = setup_r_via_overrides_with(
                &[
                    RSourceOverride::JsonKey {
                        file: "renv.lock".into(),
                        key: "R.Version".to_string(),
                    },
                    RSourceOverride::VersionFile {
                        file: "fallback.r-version".into(),
                    },
                ],
                Some(temp.path()),
                || Ok(()),
                || Ok(vec![rig_version("4.4.2")]),
                |selected, versions| {
                    assert_eq!(selected, &semver::Version::new(4, 4, 2));
                    Ok(ResolvedRSource {
                        status: RSourceStatus::Rig {
                            version: versions[0].version.clone(),
                            override_info: None,
                        },
                        r_home: Some(PathBuf::from("/tmp/r-home")),
                    })
                },
            )
            .unwrap();

            match result {
                OverrideResolution::Applied {
                    info, diagnostics, ..
                } => {
                    assert_eq!(info.provider, "version-file", "case: {name}");
                    assert_eq!(diagnostics.len(), 1, "case: {name}");
                    if name == "parse" {
                        assert!(diagnostics[0].message.contains("Failed to parse"));
                    } else {
                        assert!(diagnostics[0].message.contains("renv.lock:R.Version"));
                    }
                }
                OverrideResolution::Fallback { .. } => {
                    panic!("the fallback provider should be applied for {name}")
                }
            }
        }
    }

    #[test]
    fn overlong_version_file_value_is_not_disclosed_in_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let marker = "4".repeat(300);
        std::fs::write(temp.path().join("project.r-version"), &marker).unwrap();

        let result = setup_r_via_overrides_with(
            &[RSourceOverride::VersionFile {
                file: "project.r-version".into(),
            }],
            Some(temp.path()),
            || panic!("rig should not be queried when reading the version file fails"),
            || panic!("installed versions should not be queried when reading fails"),
            |_, _| panic!("version resolution should not be attempted"),
        )
        .unwrap();

        match result {
            OverrideResolution::Fallback { diagnostics } => {
                let diagnostics = diagnostic_text(&diagnostics);
                assert!(diagnostics.contains("exceeds 256 bytes"));
                assert!(!diagnostics.contains(&marker));
            }
            OverrideResolution::Applied { .. } => {
                panic!("an overlong version-file value must not be applied")
            }
        }
    }

    #[test]
    fn later_version_file_lines_are_not_disclosed_in_diagnostic() {
        let temp = tempfile::tempdir().unwrap();
        let private_contents = "private trailing contents that must not be disclosed";
        std::fs::write(
            temp.path().join("project.r-version"),
            format!("not-a-version\n{private_contents}\n"),
        )
        .unwrap();

        let result = setup_r_via_overrides_with(
            &[RSourceOverride::VersionFile {
                file: "project.r-version".into(),
            }],
            Some(temp.path()),
            || Ok(()),
            || Ok(vec![rig_version("4.4.2")]),
            |_, _| panic!("version resolution should not be attempted"),
        )
        .unwrap();

        match result {
            OverrideResolution::Fallback { diagnostics } => {
                let diagnostics = diagnostic_text(&diagnostics);
                assert!(!diagnostics.contains(private_contents));
            }
            OverrideResolution::Applied { .. } => {
                panic!("an invalid first line must not be applied")
            }
        }
    }

    #[test]
    fn invalid_version_markers_are_not_disclosed_for_file_providers() {
        let marker = "SUPER-SECRET-TOKEN-abc123";

        for provider in ["version-file", "toml-key", "json-key"] {
            let temp = tempfile::tempdir().unwrap();
            let overrides = if provider == "version-file" {
                std::fs::write(temp.path().join("project.r-version"), marker).unwrap();
                vec![RSourceOverride::VersionFile {
                    file: "project.r-version".into(),
                }]
            } else if provider == "toml-key" {
                std::fs::write(
                    temp.path().join("rproject.toml"),
                    format!("[project]\nr_version = \"{marker}\"\n"),
                )
                .unwrap();
                vec![RSourceOverride::TomlKey {
                    file: "rproject.toml".into(),
                    key: "project.r_version".to_string(),
                }]
            } else {
                std::fs::write(
                    temp.path().join("renv.lock"),
                    format!(r#"{{"R":{{"Version":"{marker}"}}}}"#),
                )
                .unwrap();
                vec![RSourceOverride::JsonKey {
                    file: "renv.lock".into(),
                    key: "R.Version".to_string(),
                }]
            };

            let result = setup_r_via_overrides_with(
                &overrides,
                Some(temp.path()),
                || panic!("rig should not be queried for an invalid version"),
                || panic!("installed versions should not be queried for an invalid version"),
                |_, _| panic!("version resolution should not be attempted"),
            )
            .unwrap();

            match result {
                OverrideResolution::Fallback { diagnostics } => {
                    let diagnostics = diagnostic_text(&diagnostics);
                    assert!(!diagnostics.contains(marker));
                    assert!(diagnostics.contains("could not be parsed"));
                }
                OverrideResolution::Applied { .. } => {
                    panic!("an invalid version must not be applied")
                }
            }
        }
    }

    #[test]
    fn malformed_toml_does_not_disclose_file_contents_in_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let marker = "SUPER-SECRET-TOML-CONTENTS";
        std::fs::write(
            temp.path().join("rproject.toml"),
            format!("[project]\nr_version = \"{marker}\n"),
        )
        .unwrap();

        let result = setup_r_via_overrides_with(
            &[RSourceOverride::TomlKey {
                file: "rproject.toml".into(),
                key: "project.r_version".to_string(),
            }],
            Some(temp.path()),
            || panic!("rig should not be queried for malformed TOML"),
            || panic!("installed versions should not be queried for malformed TOML"),
            |_, _| panic!("version resolution should not be attempted"),
        )
        .unwrap();

        match result {
            OverrideResolution::Fallback { diagnostics } => {
                let diagnostics = diagnostic_text(&diagnostics);
                assert!(diagnostics.contains(
                    "Warning: Failed to parse R source override file rproject.toml; trying the next R source override."
                ));
                assert!(!diagnostics.contains(marker));
            }
            OverrideResolution::Applied { .. } => {
                panic!("malformed TOML must not be applied")
            }
        }
    }

    #[test]
    fn malformed_json_does_not_disclose_file_contents_in_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let marker = "SUPER-SECRET-JSON-CONTENTS";
        std::fs::write(
            temp.path().join("renv.lock"),
            format!(r#"{{"R":{{"Version":"{marker}"}}"#),
        )
        .unwrap();

        let result = setup_r_via_overrides_with(
            &[RSourceOverride::JsonKey {
                file: "renv.lock".into(),
                key: "R.Version".to_string(),
            }],
            Some(temp.path()),
            || panic!("rig should not be queried for malformed JSON"),
            || panic!("installed versions should not be queried for malformed JSON"),
            |_, _| panic!("version resolution should not be attempted"),
        )
        .unwrap();

        match result {
            OverrideResolution::Fallback { diagnostics } => {
                let diagnostics = diagnostic_text(&diagnostics);
                assert!(diagnostics.contains(
                    "Warning: Failed to parse R source override file renv.lock; trying the next R source override."
                ));
                assert!(!diagnostics.contains(marker));
            }
            OverrideResolution::Applied { .. } => {
                panic!("malformed JSON must not be applied")
            }
        }
    }

    #[test]
    fn invalid_override_filenames_are_skipped_with_diagnostics() {
        let invalid_files = ["../x", "sub/x", r"a\b", "/absolute", "C:foo", ".", "..", ""];

        for provider in ["version-file", "toml-key", "json-key"] {
            for invalid_file in invalid_files {
                let temp = tempfile::tempdir().unwrap();
                let valid_file = if provider == "version-file" {
                    PathBuf::from("valid.r-version")
                } else if provider == "toml-key" {
                    PathBuf::from("valid.toml")
                } else {
                    PathBuf::from("valid.json")
                };
                let valid_path = temp.path().join(&valid_file);
                if provider == "version-file" {
                    std::fs::write(valid_path, "4.4.2\n").unwrap();
                } else if provider == "toml-key" {
                    std::fs::write(valid_path, "[project]\nr_version = \"4.4.2\"\n").unwrap();
                } else {
                    std::fs::write(valid_path, r#"{"R":{"Version":"4.4.2"}}"#).unwrap();
                }

                let invalid = PathBuf::from(invalid_file);
                let overrides = if provider == "version-file" {
                    vec![
                        RSourceOverride::VersionFile { file: invalid },
                        RSourceOverride::VersionFile {
                            file: valid_file.clone(),
                        },
                    ]
                } else if provider == "toml-key" {
                    vec![
                        RSourceOverride::TomlKey {
                            file: invalid,
                            key: "project.r_version".to_string(),
                        },
                        RSourceOverride::TomlKey {
                            file: valid_file.clone(),
                            key: "project.r_version".to_string(),
                        },
                    ]
                } else {
                    vec![
                        RSourceOverride::JsonKey {
                            file: invalid,
                            key: "R.Version".to_string(),
                        },
                        RSourceOverride::JsonKey {
                            file: valid_file.clone(),
                            key: "R.Version".to_string(),
                        },
                    ]
                };

                let result = setup_r_via_overrides_with(
                    &overrides,
                    Some(temp.path()),
                    || Ok(()),
                    || Ok(vec![rig_version("4.4.2")]),
                    |selected, versions| {
                        assert_eq!(selected, &semver::Version::new(4, 4, 2));
                        Ok(ResolvedRSource {
                            status: RSourceStatus::Rig {
                                version: versions[0].version.clone(),
                                override_info: None,
                            },
                            r_home: Some(PathBuf::from("/tmp/r-home")),
                        })
                    },
                )
                .unwrap();

                match result {
                    OverrideResolution::Applied {
                        info, diagnostics, ..
                    } => {
                        assert_eq!(info.file, Some(valid_file));
                        assert_eq!(diagnostics.len(), 1);
                        assert_eq!(diagnostics[0].code, "r_source_override.value_invalid");
                        assert!(
                            diagnostics[0]
                                .message
                                .contains("file must be a bare filename")
                        );
                    }
                    OverrideResolution::Fallback { .. } => {
                        panic!("the valid provider should be applied")
                    }
                }
            }
        }
    }

    #[test]
    fn rig_data_is_fetched_once_across_override_providers() {
        let temp = tempfile::tempdir().unwrap();
        let first_file = Path::new("first.r-version").to_path_buf();
        let second_file = Path::new("second.r-version").to_path_buf();
        std::fs::write(temp.path().join(&first_file), "4.3.0\n").unwrap();
        std::fs::write(temp.path().join(&second_file), "4.4.2\n").unwrap();

        let rig_available_calls = Arc::new(AtomicUsize::new(0));
        let list_versions_calls = Arc::new(AtomicUsize::new(0));
        let rig_available_calls_for_closure = Arc::clone(&rig_available_calls);
        let list_versions_calls_for_closure = Arc::clone(&list_versions_calls);

        let result = setup_r_via_overrides_with(
            &[
                RSourceOverride::VersionFile { file: first_file },
                RSourceOverride::VersionFile {
                    file: second_file.clone(),
                },
            ],
            Some(temp.path()),
            move || {
                rig_available_calls_for_closure.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
            move || {
                list_versions_calls_for_closure.fetch_add(1, Ordering::Relaxed);
                Ok(vec![rig_version("4.4.2")])
            },
            |selected, versions| {
                assert_eq!(selected, &semver::Version::new(4, 4, 2));
                Ok(ResolvedRSource {
                    status: RSourceStatus::Rig {
                        version: versions[0].version.clone(),
                        override_info: None,
                    },
                    r_home: Some(PathBuf::from("/tmp/r-home")),
                })
            },
        )
        .unwrap();

        assert!(matches!(result, OverrideResolution::Applied { .. }));
        assert_eq!(rig_available_calls.load(Ordering::Relaxed), 1);
        assert_eq!(list_versions_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn missing_override_files_do_not_fetch_rig_data() {
        let temp = tempfile::tempdir().unwrap();
        let rig_available_calls = Arc::new(AtomicUsize::new(0));
        let list_versions_calls = Arc::new(AtomicUsize::new(0));
        let rig_available_calls_for_closure = Arc::clone(&rig_available_calls);
        let list_versions_calls_for_closure = Arc::clone(&list_versions_calls);

        let result = setup_r_via_overrides_with(
            &[
                RSourceOverride::VersionFile {
                    file: "missing-first.r-version".into(),
                },
                RSourceOverride::VersionFile {
                    file: "missing-second.r-version".into(),
                },
            ],
            Some(temp.path()),
            move || {
                rig_available_calls_for_closure.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
            move || {
                list_versions_calls_for_closure.fetch_add(1, Ordering::Relaxed);
                Ok(Vec::new())
            },
            |_, _| panic!("R version resolution should not be attempted"),
        );

        assert!(result.is_none());
        assert_eq!(rig_available_calls.load(Ordering::Relaxed), 0);
        assert_eq!(list_versions_calls.load(Ordering::Relaxed), 0);
    }

    fn rig_version(version: &str) -> external::rig::RigVersion {
        external::rig::RigVersion {
            name: version.to_owned(),
            default: true,
            version: version.to_owned(),
            aliases: Vec::new(),
            path: format!("/opt/R/{version}"),
            binary: format!("/opt/R/{version}/bin/R"),
        }
    }
}
