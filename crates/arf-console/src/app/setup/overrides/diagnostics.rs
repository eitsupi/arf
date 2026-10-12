//! Diagnostics produced while evaluating R source override providers.

use super::provider::ProviderSpec;
use crate::app::setup::pixi::PixiFailure;
use crate::app::setup::r_source::{RSourceDiagnostic, warning};
use crate::app::setup::rig::ResolvedRSource;
use crate::app::setup::rig_project::RigProjectFailure;
use crate::external;
use crate::rversion;
use std::path::{Path, PathBuf};

const CODE_PREFIX: &str = "r_source_override";

fn code(suffix: &str) -> String {
    format!("{CODE_PREFIX}.{suffix}")
}

pub(super) fn pixi_failure_diagnostic(error: &PixiFailure) -> Option<RSourceDiagnostic> {
    let code = code(error.code_suffix());
    match error {
        PixiFailure::NotActive {
            workspace_manifest: Some(path),
        } => Some(warning(
            code,
            "Warning: A Pixi workspace was found, but its environment is not active. Start arf with `pixi run arf`; trying the next R source override.",
            Some(path.clone()),
        )),
        PixiFailure::NotActive {
            workspace_manifest: None,
        } => None,
        PixiFailure::EnvironmentInvalid { reason, .. } => Some(warning(
            code,
            format!(
                "Warning: The active Pixi environment is invalid: {reason}; trying the next R source override."
            ),
            None,
        )),
        PixiFailure::RUnavailable { reason, .. } => Some(warning(
            code,
            format!(
                "Warning: R from the active Pixi environment is unavailable: {reason}; trying the next R source override."
            ),
            None,
        )),
    }
}

pub(super) fn rig_project_failure_diagnostic(error: &RigProjectFailure) -> RSourceDiagnostic {
    let path = match error {
        RigProjectFailure::ProjectNotFound { path } => Some(path.clone()),
        RigProjectFailure::StatusUnavailable { path, .. }
        | RigProjectFailure::StatusInvalid { path, .. }
        | RigProjectFailure::NotSynced { path, .. }
        | RigProjectFailure::RUnavailable { path, .. } => path.clone(),
    };
    warning(
        code(error.code_suffix()),
        format!("Warning: {}; trying the next R source override.", error),
        path,
    )
}

pub(super) fn invalid_override_file_warning(provider: &str, file: &Path) -> RSourceDiagnostic {
    warning(
        "r_source_override.value_invalid",
        format!(
            "Warning: R source override provider '{provider}' has invalid file '{}'; file must be a bare filename; trying the next R source override.",
            file.display()
        ),
        Some(file.to_path_buf()),
    )
}

pub(super) fn rig_unavailable_warning(error: &external::rig::RigError) -> RSourceDiagnostic {
    let message = match error {
        external::rig::RigError::NotInstalled => {
            "Warning: rig is not installed, so the R source override cannot be resolved.\n         Install rig from https://github.com/r-lib/rig or use \"auto\".\n         Trying the next R source override."
                .to_string()
        }
        external::rig::RigError::CommandFailed(reason) => format!(
            "Warning: rig is installed but could not be run, so the R source override cannot be resolved: {reason}.\n         Fix the rig installation or command, then restart arf.\n         Trying the next R source override."
        ),
        error => format!(
            "Warning: rig availability could not be checked ({error}), so the R source override cannot be resolved.\n         Fix the rig installation or command, then restart arf.\n         Trying the next R source override."
        ),
    };

    warning("r_source_override.rig_unavailable", message, None)
}

pub(super) fn fallback_warning() -> RSourceDiagnostic {
    warning(
        "r_source_override.fallback",
        "Warning: All R source overrides failed.\n         Falling back to startup.r_source.",
        None,
    )
}

pub(super) fn not_installed_warning(
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

pub(super) fn empty_version_warning(provider: &ProviderSpec<'_>) -> RSourceDiagnostic {
    warning(
        "r_source_override.value_invalid",
        format!(
            "Warning: {} contains an empty R version specification; trying the next R source override.",
            provider.location()
        ),
        provider.file_path(),
    )
}

pub(super) fn unparsable_version_warning(provider: &ProviderSpec<'_>) -> RSourceDiagnostic {
    warning(
        "r_source_override.value_invalid",
        format!(
            "Warning: {} contains an R version specification that could not be parsed; trying the next R source override.",
            provider.location()
        ),
        provider.file_path(),
    )
}

pub(super) fn named_version_unsupported_warning(
    provider: &ProviderSpec<'_>,
    name: &str,
) -> RSourceDiagnostic {
    warning(
        "r_source_override.provider_unsupported",
        format!(
            r#"Warning: R version "{name}" from {} is unsupported in the R source override path; trying the next R source override."#,
            provider.location()
        ),
        provider.file_path(),
    )
}

pub(super) fn installed_versions_unavailable_warning(
    provider: &ProviderSpec<'_>,
    error: &external::rig::RigError,
) -> RSourceDiagnostic {
    warning(
        "r_source_override.resolution_failed",
        format!(
            "Warning: Could not inspect installed R versions for {}: {error}; trying the next R source override.",
            provider.location()
        ),
        provider.file_path(),
    )
}

pub(super) fn missing_r_home_warning(provider: &ProviderSpec<'_>) -> RSourceDiagnostic {
    warning(
        "r_source_override.resolution_failed",
        "Warning: R source override resolved without an R_HOME; trying the next R source override.",
        provider.file_path(),
    )
}

pub(super) fn unsupported_status_warning(
    provider: &ProviderSpec<'_>,
    resolved: &ResolvedRSource,
) -> RSourceDiagnostic {
    warning(
        "r_source_override.provider_unsupported",
        format!(
            "Warning: R source override {} resolved to an unsupported R source status ({resolved:?}); trying the next R source override.",
            provider.location()
        ),
        provider.file_path(),
    )
}

pub(super) fn version_resolution_failed_warning(
    provider: &ProviderSpec<'_>,
    version: &str,
    error: &anyhow::Error,
) -> RSourceDiagnostic {
    warning(
        "r_source_override.resolution_failed",
        format!(
            r#"Warning: Failed to use R version "{version}" from {}: {error}; trying the next R source override."#,
            provider.location()
        ),
        provider.file_path(),
    )
}
