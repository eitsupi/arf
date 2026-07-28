//! R setup, script execution mode, and session ID creation.

use crate::app::config_load::load_config_or_warn;
use crate::cli::Cli;
use crate::config;
use crate::config::{
    Config, RSource, RSourceMode, RSourceOverride, RSourceOverrideInfo, RSourceStatus,
};
use crate::external;
use crate::rversion;
use anyhow::{Context, Result};
use reedline::Reedline;
use std::fs;
#[cfg(windows)]
use std::path::PathBuf;

/// Run in script execution mode (non-interactive).
pub(crate) fn run_script(cli: &Cli) -> Result<()> {
    // Load configuration (from file or default)
    let config = load_config_or_warn(cli.config.as_ref());

    // Set up R based on r_source config (with optional CLI override)
    let resolution = setup_r(
        &config.startup.r_source,
        &config.experimental.r_source_overrides,
        cli.r_home.as_deref(),
        cli.r_version.as_deref(),
        cli.no_r_source_overrides,
    )?;
    resolution.emit_diagnostics();
    if let Some(notice) = script_override_notice(&resolution) {
        eprintln!("{notice}");
    }

    // Ensure LD_LIBRARY_PATH includes R library directory
    if let Err(e) = arf_libr::ensure_ld_library_path() {
        log::warn!("Could not set LD_LIBRARY_PATH: {}", e);
    }

    // Generate R initialization arguments from CLI flags
    let r_args = cli.r_args();
    let r_args_refs: Vec<&str> = r_args.iter().map(|s| s.as_str()).collect();

    // Initialize R with CLI-specified flags
    unsafe {
        arf_libr::initialize_r_with_args(&r_args_refs).context("Failed to initialize R")?;
    }

    // Source R profile files (Windows only)
    #[cfg(windows)]
    source_r_profiles(&r_args);

    // Get the code to execute
    let code = if let Some(eval_code) = &cli.eval {
        eval_code.clone()
    } else if let Some(script_path) = cli.script_file() {
        if script_path == std::path::Path::new("-") {
            use std::io::Read;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("Failed to read from stdin")?;
            buf
        } else {
            fs::read_to_string(script_path)
                .with_context(|| format!("Failed to read script file: {}", script_path.display()))?
        }
    } else {
        // Should not happen - we checked script_mode earlier
        return Ok(());
    };

    // Evaluate the code - use reprex mode if enabled (CLI or config)
    let reprex_enabled = cli.reprex || config.startup.mode.reprex;
    if reprex_enabled {
        // In reprex mode, echo source code before each result
        match arf_harp::eval_string_reprex(&code, &config.mode.reprex.comment) {
            Ok(_) => Ok(()),
            Err(e) => {
                eprintln!("{}", e);
                Ok(())
            }
        }
    } else {
        // Normal script execution
        match arf_harp::eval_string(&code) {
            Ok(_) => Ok(()),
            Err(e) => {
                eprintln!("{}", e);
                Ok(())
            }
        }
    }
}

/// Set up R based on r_source configuration.
///
/// CLI options override config in this order:
/// 1. `cli_r_home` - explicit R_HOME path
/// 2. `cli_version` - rig version specification
/// 3. Experimental directory-level R source overrides
/// 4. Config `r_source` setting
///
/// The state of override resolution for reporting to users and headless clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RSourceOverrideState {
    /// An override selected the active R installation.
    Applied,
    /// No R source override providers are configured.
    NotConfigured,
    /// No configured provider matched a file in the current directory.
    NoMatch,
    /// At least one provider was evaluated but no override could be applied.
    Failed,
    /// Override resolution was disabled by the CLI.
    Disabled,
    /// A CLI R source took precedence over overrides.
    ShadowedByCli,
}

impl RSourceOverrideState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Applied => "applied",
            Self::NotConfigured => "not_configured",
            Self::NoMatch => "no_match",
            Self::Failed => "failed",
            Self::Disabled => "disabled",
            Self::ShadowedByCli => "shadowed_by_cli",
        }
    }
}

/// The result of resolving the configured R source and any directory override.
#[derive(Debug, Clone)]
pub(crate) struct RSourceResolutionReport {
    pub(crate) status: RSourceStatus,
    pub(crate) provider: Option<String>,
    pub(crate) file: Option<std::path::PathBuf>,
    pub(crate) key: Option<String>,
    pub(crate) requested_version: Option<String>,
    pub(crate) resolved_version: Option<String>,
    pub(crate) diagnostics: Vec<String>,
    pub(crate) override_state: RSourceOverrideState,
}

impl RSourceResolutionReport {
    fn from_status(status: RSourceStatus, override_state: RSourceOverrideState) -> Self {
        Self {
            status,
            provider: None,
            file: None,
            key: None,
            requested_version: None,
            resolved_version: None,
            diagnostics: Vec::new(),
            override_state,
        }
    }

    fn applied(status: RSourceStatus, info: RSourceOverrideInfo, diagnostics: Vec<String>) -> Self {
        Self {
            status,
            provider: Some(info.provider),
            file: info.file,
            key: info.key,
            requested_version: Some(info.requested_version),
            resolved_version: Some(info.resolved_version),
            diagnostics,
            override_state: RSourceOverrideState::Applied,
        }
    }

    /// Emit all override diagnostics as one warning block for normal startup.
    pub(crate) fn emit_diagnostics(&self) {
        if self.diagnostics.is_empty() {
            return;
        }
        let block = self.diagnostics.join("\n");
        eprintln!("{block}");
        log::warn!("{block}");
    }

    pub(crate) fn override_info(&self) -> Option<RSourceOverrideInfo> {
        Some(RSourceOverrideInfo {
            provider: self.provider.clone()?,
            file: self.file.clone(),
            key: self.key.clone(),
            requested_version: self.requested_version.clone()?,
            resolved_version: self.resolved_version.clone()?,
        })
    }
}

/// Set up R and return a resolution report for display and feature gating.
pub(crate) fn setup_r(
    r_source: &RSource,
    r_source_overrides: &[RSourceOverride],
    cli_r_home: Option<&std::path::Path>,
    cli_version: Option<&str>,
    no_r_source_overrides: bool,
) -> Result<RSourceResolutionReport> {
    // CLI --r-home overrides everything. An explicit disable still controls
    // the reported override state without changing the selected R source.
    if let Some(path) = cli_r_home {
        if !path.exists() {
            anyhow::bail!(
                "R_HOME path does not exist: {}\n\
                 Check your --r-home argument.",
                path.display()
            );
        }
        // Resolve R_HOME: if path looks like an installation prefix (has bin/R),
        // run `bin/R RHOME` to get the actual R_HOME directory
        let r_home = resolve_r_home_from_path(path)?;
        log::info!("Using R from --r-home: {}", r_home.display());
        // SAFETY: We're single-threaded at this point during startup
        unsafe { std::env::set_var("R_HOME", &r_home) };
        return Ok(RSourceResolutionReport::from_status(
            RSourceStatus::ExplicitPath { path: r_home },
            if no_r_source_overrides {
                RSourceOverrideState::Disabled
            } else {
                RSourceOverrideState::ShadowedByCli
            },
        ));
    }

    // CLI --with-r-version overrides config (uses rig)
    if let Some(version) = cli_version {
        return setup_r_via_rig(version).map(|status| {
            RSourceResolutionReport::from_status(
                status,
                if no_r_source_overrides {
                    RSourceOverrideState::Disabled
                } else {
                    RSourceOverrideState::ShadowedByCli
                },
            )
        });
    }

    if no_r_source_overrides {
        return setup_r_fallback(r_source, RSourceOverrideState::Disabled);
    }

    if let Some(result) = setup_r_via_overrides(r_source_overrides) {
        match result {
            OverrideResolution::Applied {
                status,
                info,
                diagnostics,
            } => return Ok(RSourceResolutionReport::applied(*status, info, diagnostics)),
            OverrideResolution::Fallback { diagnostics } => {
                let mut report = setup_r_fallback(r_source, RSourceOverrideState::Failed)?;
                report.diagnostics = diagnostics;
                return Ok(report);
            }
        }
    }

    let override_state = if r_source_overrides.is_empty() {
        RSourceOverrideState::NotConfigured
    } else {
        RSourceOverrideState::NoMatch
    };
    setup_r_fallback(r_source, override_state)
}

fn setup_r_fallback(
    r_source: &RSource,
    override_state: RSourceOverrideState,
) -> Result<RSourceResolutionReport> {
    let status = match r_source {
        RSource::Mode(RSourceMode::Auto) => {
            // Auto mode: try rig if available, otherwise use PATH
            if external::rig::rig_available() {
                match external::rig::resolve_version("default") {
                    Ok(resolved) => {
                        log::info!("Using rig default R version: {}", resolved.version);
                        // SAFETY: We're single-threaded at this point during startup
                        unsafe { std::env::set_var("R_HOME", &resolved.r_home) };
                        RSourceStatus::Rig {
                            version: resolved.version,
                            override_info: None,
                        }
                    }
                    Err(e) => {
                        log::debug!("Could not get rig default version: {}", e);
                        log::info!("Using R from PATH");
                        RSourceStatus::Path
                    }
                }
            } else {
                log::info!("Using R from PATH (rig not available)");
                RSourceStatus::Path
            }
        }
        RSource::Mode(RSourceMode::Rig) => {
            // Rig mode: require rig
            if !external::rig::rig_available() {
                anyhow::bail!(
                    r#"r_source = "rig" but rig is not installed.
Install rig from https://github.com/r-lib/rig or use "auto"."#
                );
            }
            match external::rig::resolve_version("default") {
                Ok(resolved) => {
                    log::info!("Using rig default R version: {}", resolved.version);
                    // SAFETY: We're single-threaded at this point during startup
                    unsafe { std::env::set_var("R_HOME", &resolved.r_home) };
                    RSourceStatus::Rig {
                        version: resolved.version,
                        override_info: None,
                    }
                }
                Err(e) => {
                    anyhow::bail!("Failed to get rig default R version: {}", e);
                }
            }
        }
        RSource::Path { path } => {
            // Explicit path mode
            if !path.exists() {
                anyhow::bail!(
                    "R_HOME path does not exist: {}\n\
                     Check your r_source configuration.",
                    path.display()
                );
            }
            log::info!("Using R from explicit path: {}", path.display());
            // SAFETY: We're single-threaded at this point during startup
            unsafe { std::env::set_var("R_HOME", path) };
            RSourceStatus::ExplicitPath { path: path.clone() }
        }
    };

    Ok(RSourceResolutionReport::from_status(status, override_state))
}

enum OverrideResolution {
    Applied {
        status: Box<RSourceStatus>,
        info: RSourceOverrideInfo,
        diagnostics: Vec<String>,
    },
    Fallback {
        diagnostics: Vec<String>,
    },
}

/// Try the configured directory-level R source overrides in priority order.
fn setup_r_via_overrides(overrides: &[RSourceOverride]) -> Option<OverrideResolution> {
    setup_r_via_overrides_with(
        overrides,
        external::rig::rig_available,
        external::rig::list_versions,
        setup_r_via_selected_rig_version,
    )
}

fn setup_r_via_overrides_with<FAvailable, FList, FResolve>(
    overrides: &[RSourceOverride],
    rig_available: FAvailable,
    list_versions: FList,
    resolve_selected_rig_version: FResolve,
) -> Option<OverrideResolution>
where
    FAvailable: Fn() -> bool,
    FList: Fn() -> std::result::Result<Vec<external::rig::RigVersion>, external::rig::RigError>,
    FResolve: Fn(&semver::Version, &[external::rig::RigVersion]) -> Result<RSourceStatus>,
{
    let mut diagnostics = Vec::new();
    let mut evaluated_provider = false;
    let mut rig_available_cache = None;
    let mut installed_versions_cache = None;

    for source in overrides {
        let provider = override_provider_name(source);
        let version = match source {
            RSourceOverride::Pixi => {
                evaluated_provider = true;
                diagnostics.push(format!(
                    "Warning: R source override provider '{provider}' is not implemented; trying the next R source override."
                ));
                continue;
            }
            RSourceOverride::VersionFile { file } => match rversion::read_version_file(file) {
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
                    diagnostics.push(format!(
                        "Warning: Failed to read R version override file {}: {error}; trying the next R source override.",
                        file.display()
                    ));
                    continue;
                }
            },
            RSourceOverride::TomlKey { file, key } => match rversion::read_toml_key(file, key) {
                Ok(version) => {
                    evaluated_provider = true;
                    version
                }
                Err(error) if error.is_not_found() => {
                    log::debug!("R source override file {} is not present", file.display());
                    continue;
                }
                Err(
                    rversion::TomlKeyError::MissingKey(_) | rversion::TomlKeyError::NotString(_),
                ) => {
                    evaluated_provider = true;
                    diagnostics.push(format!(
                        "Warning: {}:{} does not contain the configured R version key; trying the next R source override.",
                        file.display(),
                        key
                    ));
                    continue;
                }
                Err(rversion::TomlKeyError::Parse(error)) => {
                    evaluated_provider = true;
                    diagnostics.push(format!(
                        "Warning: Failed to parse R source override file {}: {error}; trying the next R source override.",
                        file.display()
                    ));
                    continue;
                }
                Err(error) => {
                    evaluated_provider = true;
                    diagnostics.push(format!(
                        "Warning: Failed to read R version from TOML key '{}' in {}: {error}; trying the next R source override.",
                        key,
                        file.display()
                    ));
                    continue;
                }
            },
        };

        let trimmed_version = version.trim().to_owned();
        let spec = match rversion::VersionSpec::parse(&trimmed_version) {
            Ok(spec) => spec,
            Err(error) => {
                diagnostics.push(format!(
                    "Warning: {} contains invalid R version \"{}\" ({error}); trying the next R source override.",
                    override_location(source),
                    trimmed_version
                ));
                continue;
            }
        };

        if let rversion::VersionSpec::Named(name) = &spec {
            diagnostics.push(format!(
                "Warning: R version \"{name}\" from {} is unsupported in the R source override path; trying the next R source override.",
                override_location(source)
            ));
            continue;
        }

        let rig_is_available = *rig_available_cache.get_or_insert_with(&rig_available);
        if !rig_is_available {
            diagnostics.push(rig_unavailable_warning());
            return Some(OverrideResolution::Fallback { diagnostics });
        }

        let installed = if let Some(installed) = installed_versions_cache.as_ref() {
            installed
        } else {
            match list_versions() {
                Ok(versions) => installed_versions_cache.insert(versions),
                Err(error) => {
                    diagnostics.push(format!(
                        "Warning: Could not inspect installed R versions for {}: {error}; falling back to startup.r_source.",
                        override_location(source)
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
            ));
            continue;
        };

        match resolve_selected_rig_version(selected, installed) {
            Ok(RSourceStatus::Rig { version, .. }) => {
                let info = RSourceOverrideInfo {
                    provider: provider.to_owned(),
                    file: override_file(source),
                    key: override_key(source),
                    requested_version: trimmed_version,
                    resolved_version: version.clone(),
                };
                return Some(OverrideResolution::Applied {
                    status: RSourceStatus::Rig {
                        version,
                        override_info: Some(info.clone()),
                    }
                    .into(),
                    info,
                    diagnostics,
                });
            }
            Ok(status) => {
                diagnostics.push(format!(
                    "Warning: R source override {} resolved to an unsupported R source status ({status:?}); falling back to startup.r_source.",
                    override_location(source)
                ));
                return Some(OverrideResolution::Fallback { diagnostics });
            }
            Err(error) => {
                diagnostics.push(format!(
                    "Warning: Failed to use R version \"{}\" from {}: {error}; trying the next R source override.",
                    trimmed_version,
                    override_location(source)
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

fn script_override_notice(resolution: &RSourceResolutionReport) -> Option<String> {
    resolution
        .override_info()
        .map(|info| format!("# R source override: {}", info.display()))
}

fn override_provider_name(source: &RSourceOverride) -> &'static str {
    match source {
        RSourceOverride::Pixi => "pixi",
        RSourceOverride::VersionFile { .. } => "version-file",
        RSourceOverride::TomlKey { .. } => "toml-key",
    }
}

fn override_file(source: &RSourceOverride) -> Option<std::path::PathBuf> {
    match source {
        RSourceOverride::Pixi => None,
        RSourceOverride::VersionFile { file } | RSourceOverride::TomlKey { file, .. } => {
            Some(file.clone())
        }
    }
}

fn override_key(source: &RSourceOverride) -> Option<String> {
    match source {
        RSourceOverride::TomlKey { key, .. } => Some(key.clone()),
        RSourceOverride::Pixi | RSourceOverride::VersionFile { .. } => None,
    }
}

fn override_location(source: &RSourceOverride) -> String {
    match source {
        RSourceOverride::Pixi => "pixi".to_owned(),
        RSourceOverride::VersionFile { file } => file.display().to_string(),
        RSourceOverride::TomlKey { file, key } => format!("{}:{}", file.display(), key),
    }
}

fn rig_unavailable_warning() -> String {
    "Warning: rig is not installed, so the R source override cannot be resolved.\n         Install rig from https://github.com/r-lib/rig or use \"auto\".\n         Falling back to startup.r_source."
        .to_owned()
}

fn fallback_warning() -> String {
    "Warning: All R source overrides failed.\n         Falling back to startup.r_source.".to_owned()
}

fn not_installed_warning(
    provider: &str,
    location: &str,
    version: &str,
    spec: &rversion::VersionSpec,
) -> String {
    if spec.is_concrete_version() {
        format!(
            "Warning: R source override provider '{provider}' at {location} requested R version \"{version}\", which is not installed.\n         Install it with rig add {version}, then restart arf.\n         Trying the next R source override."
        )
    } else {
        format!(
            "Warning: R source override provider '{provider}' at {location} has no installed R version matching specification \"{version}\".\n         Install a matching R version with rig, then restart arf.\n         Trying the next R source override."
        )
    }
}

/// Resolve R_HOME from a user-provided path.
///
/// The path can be either:
/// - An installation prefix (e.g., `/opt/R/4.5.2`) containing `bin/R`
/// - The actual R_HOME directory (e.g., `/opt/R/4.5.2/lib/R`)
///
/// If the path contains `bin/R`, we run it with `RHOME` to get the actual R_HOME.
fn resolve_r_home_from_path(path: &std::path::Path) -> Result<std::path::PathBuf> {
    // Check if this looks like an installation prefix (has bin/R)
    let r_binary = path.join("bin").join("R");
    if r_binary.exists() {
        // Run `bin/R RHOME` to get the actual R_HOME
        let output = std::process::Command::new(&r_binary)
            .arg("RHOME")
            .output()
            .with_context(|| format!("Failed to run {} RHOME", r_binary.display()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("{} RHOME failed: {}", r_binary.display(), stderr);
        }

        let r_home = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if r_home.is_empty() {
            anyhow::bail!("{} RHOME returned empty result", r_binary.display());
        }

        log::debug!(
            "Resolved R_HOME from installation prefix: {} -> {}",
            path.display(),
            r_home
        );
        return Ok(std::path::PathBuf::from(r_home));
    }

    // Assume the path is already R_HOME
    // Validate by checking for etc/Renviron
    let renviron = path.join("etc").join("Renviron");
    if !renviron.exists() {
        log::warn!(
            "Path {} does not look like R_HOME (missing etc/Renviron). \
             Consider providing the installation prefix instead.",
            path.display()
        );
    }

    Ok(path.to_path_buf())
}

/// Set up R via rig with a specific version (used for CLI --with-r-version).
fn setup_r_via_rig(version_spec: &str) -> Result<RSourceStatus> {
    if !external::rig::rig_available() {
        anyhow::bail!(
            "--with-r-version requires rig to be installed.\n\
             Install rig from https://github.com/r-lib/rig"
        );
    }

    match external::rig::resolve_version(version_spec) {
        Ok(resolved) => apply_rig_resolution(resolved),
        Err(e) => {
            anyhow::bail!("Failed to resolve R version '{}': {}", version_spec, e);
        }
    }
}

/// Set up R from the exact semantic version selected for an override.
///
/// The override resolver has already selected a version from rig's reported
/// version fields. Re-resolving its string would allow a rig name or alias to
/// select a different installation.
fn setup_r_via_selected_rig_version(
    selected: &semver::Version,
    versions: &[external::rig::RigVersion],
) -> Result<RSourceStatus> {
    match external::rig::resolve_selected_version_from_versions(selected, versions) {
        Ok(resolved) => apply_rig_resolution(resolved),
        Err(error) => anyhow::bail!("Failed to resolve R version '{}': {}", selected, error),
    }
}

fn apply_rig_resolution(resolved: external::rig::ResolvedVersion) -> Result<RSourceStatus> {
    log::info!(
        "Using R version {} from {}",
        resolved.version,
        resolved.r_home
    );
    // SAFETY: We're single-threaded at this point during startup
    unsafe { std::env::set_var("R_HOME", &resolved.r_home) };
    Ok(RSourceStatus::Rig {
        version: resolved.version,
        override_info: None,
    })
}

/// Source R profile files after R initialization.
///
/// This handles loading of:
/// - Site-level Rprofile.site (unless --no-site-file or --vanilla)
/// - User-level .Rprofile (unless --no-init-file or --vanilla)
///
/// On Windows, R's built-in profile loading is disabled during initialization
/// for compatibility with `globalCallingHandlers()`, so we must manually
/// source these files here.
#[cfg(windows)]
pub(crate) fn source_r_profiles(r_args: &[String]) {
    // Fix .Platform$GUI before any R profiles or packages are loaded.
    // See: https://github.com/eitsupi/arf/issues/168
    arf_harp::override_platform_gui();

    // Get R_HOME from environment (set earlier in setup_r)
    let r_home = match std::env::var("R_HOME") {
        Ok(path) => PathBuf::from(path),
        Err(_) => {
            log::warn!("R_HOME not set, skipping R profile sourcing");
            return;
        }
    };

    // Source site-level R profile unless --no-site-file or --vanilla
    if !arf_harp::should_ignore_site_r_profile(r_args) {
        arf_harp::source_site_r_profile(&r_home);
    } else {
        log::trace!("Skipping site R profile (--no-site-file or --vanilla)");
    }

    // Source user-level R profile unless --no-init-file or --vanilla
    if !arf_harp::should_ignore_user_r_profile(r_args) {
        arf_harp::source_user_r_profile();
    } else {
        log::trace!("Skipping user R profile (--no-init-file or --vanilla)");
    }

    // Call .First() then .First.sys() to match R's documented startup sequence
    // (see `?Startup`). After profiles are loaded:
    //   1. .First()     — user hook defined in .Rprofile (e.g. vscode-R session watcher)
    //   2. .First.sys() — base package hook that loads default packages (utils, grDevices, ...)
    // On Windows we source profiles manually (profiles disabled in setup_Rmainloop for
    // globalCallingHandlers compatibility), so we must call these hooks manually too.
    arf_harp::call_dot_first();
    arf_harp::call_dot_first_sys();
}

/// Generate a history session ID when history is enabled and a history directory
/// is available, or `None` otherwise.
///
/// This ensures IPC/session JSON does not misleadingly advertise history isolation
/// when no history backend is configured.
pub(crate) fn create_session_id(config: &Config) -> Option<reedline::HistorySessionId> {
    if config.history.disabled {
        return None;
    }
    // Check that a history directory is actually resolvable, matching the logic
    // in Repl::r_history_path() / shell_history_path().
    if config.history.dir.is_none() && config::history_dir().is_none() {
        return None;
    }
    Reedline::create_history_session_id()
}

#[cfg(test)]
mod session_id_tests {
    use super::*;

    #[test]
    fn test_create_session_id_when_history_enabled() {
        let mut config = Config::default();
        // Ensure a history dir is available by setting it explicitly
        config.history.dir = Some(std::env::temp_dir());
        assert!(!config.history.disabled);
        let id = create_session_id(&config);
        assert!(
            id.is_some(),
            "should generate session ID when history is enabled"
        );
    }

    #[test]
    fn test_create_session_id_when_history_disabled() {
        let mut config = Config::default();
        config.history.disabled = true;
        let id = create_session_id(&config);
        assert!(id.is_none(), "should be None when history is disabled");
    }

    #[test]
    fn test_create_session_id_respects_default_history_dir() {
        // With default config (history.dir = None), session ID depends on
        // whether the platform provides a data directory via history_dir().
        let config = Config::default();
        assert!(!config.history.disabled);
        assert!(config.history.dir.is_none());
        let id = create_session_id(&config);
        // On most platforms history_dir() returns Some, so session ID is generated.
        // On exotic platforms where it returns None, session ID should be None.
        assert_eq!(id.is_some(), config::history_dir().is_some());
    }
}

#[cfg(test)]
mod r_source_override_tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn path_source(path: &Path) -> RSource {
        RSource::Path {
            path: path.to_path_buf(),
        }
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
                file: temp.path().join(".r-version"),
            }],
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
        let report = setup_r(&path_source(temp.path()), &[], None, None, false).unwrap();

        assert_eq!(report.override_state, RSourceOverrideState::NotConfigured);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn disabled_override_resolution_has_no_diagnostics() {
        let temp = tempfile::tempdir().unwrap();
        let report = setup_r(
            &path_source(temp.path()),
            &[RSourceOverride::Pixi],
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
                file: temp.path().join("rproject.toml"),
                key: "project.r_version".to_string(),
            }],
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
            None,
            None,
            false,
        )
        .unwrap();

        assert_eq!(report.override_state, RSourceOverrideState::Failed);
        assert_eq!(report.diagnostics.len(), 2);
        assert!(report.diagnostics[0].contains("pixi"));
        assert!(report.diagnostics[1].contains("Falling back to startup.r_source"));
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
            rig_unavailable_warning(),
            "Warning: rig is not installed, so the R source override cannot be resolved.\n         Install rig from https://github.com/r-lib/rig or use \"auto\".\n         Falling back to startup.r_source."
        );
    }

    #[test]
    fn range_not_installed_warning_does_not_suggest_rig_add() {
        let spec = rversion::VersionSpec::parse(">=4.3, <5.0").unwrap();
        let warning = not_installed_warning(
            "toml-key",
            "rproject.toml:project.r_version",
            ">=4.3, <5.0",
            &spec,
        );

        assert!(warning.contains("Install a matching R version with rig"));
        assert!(!warning.contains("rig add"));
        assert!(warning.contains("toml-key"));
        assert!(warning.contains("rproject.toml:project.r_version"));
        assert!(!warning.contains("Falling back to startup.r_source"));
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
        let first_file = temp.path().join("first.r-version");
        let second_file = temp.path().join("second.r-version");
        std::fs::write(&first_file, "4.3.0\n").unwrap();
        std::fs::write(&second_file, "4.4.2\n").unwrap();

        let result = setup_r_via_overrides_with(
            &[
                RSourceOverride::VersionFile { file: first_file },
                RSourceOverride::VersionFile {
                    file: second_file.clone(),
                },
            ],
            || true,
            || Ok(vec![rig_version("4.4.2")]),
            |selected, versions| {
                assert_eq!(selected, &semver::Version::new(4, 4, 2));
                assert_eq!(versions[0].version, "4.4.2");
                Ok(RSourceStatus::Rig {
                    version: versions[0].version.clone(),
                    override_info: None,
                })
            },
        )
        .unwrap();

        match result {
            OverrideResolution::Applied {
                status,
                info,
                diagnostics,
            } => {
                assert!(matches!(*status, RSourceStatus::Rig { .. }));
                assert_eq!(info.file, Some(second_file));
                assert_eq!(info.resolved_version, "4.4.2");
                assert_eq!(diagnostics.len(), 1);
                assert!(diagnostics[0].contains("4.3.0"));
                assert!(diagnostics[0].contains("Trying the next R source override"));
                assert!(!diagnostics[0].contains("Falling back to startup.r_source"));
            }
            OverrideResolution::Fallback { .. } => {
                panic!("the installed provider should be applied")
            }
        }
    }

    #[test]
    fn rig_data_is_fetched_once_across_override_providers() {
        let temp = tempfile::tempdir().unwrap();
        let first_file = temp.path().join("first.r-version");
        let second_file = temp.path().join("second.r-version");
        std::fs::write(&first_file, "4.3.0\n").unwrap();
        std::fs::write(&second_file, "4.4.2\n").unwrap();

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
            move || {
                rig_available_calls_for_closure.fetch_add(1, Ordering::Relaxed);
                true
            },
            move || {
                list_versions_calls_for_closure.fetch_add(1, Ordering::Relaxed);
                Ok(vec![rig_version("4.4.2")])
            },
            |selected, versions| {
                assert_eq!(selected, &semver::Version::new(4, 4, 2));
                Ok(RSourceStatus::Rig {
                    version: versions[0].version.clone(),
                    override_info: None,
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
                    file: temp.path().join("missing-first.r-version"),
                },
                RSourceOverride::VersionFile {
                    file: temp.path().join("missing-second.r-version"),
                },
            ],
            move || {
                rig_available_calls_for_closure.fetch_add(1, Ordering::Relaxed);
                true
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
