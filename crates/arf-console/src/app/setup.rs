//! R setup, script execution mode, and session ID creation.

use crate::app::config_load::load_config_or_warn;
use crate::cli::Cli;
use crate::config;
use crate::config::{Config, RSource, RSourceMode, RSourceStatus};
use crate::external;
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
    setup_r(
        &config.startup.r_source,
        cli.r_home.as_deref(),
        cli.r_version.as_deref(),
    )?;

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
/// 3. Config `r_source` setting
///
/// Returns an `RSourceStatus` describing how R was resolved (for display and feature gating).
pub(crate) fn setup_r(
    r_source: &RSource,
    cli_r_home: Option<&std::path::Path>,
    cli_version: Option<&str>,
) -> Result<RSourceStatus> {
    // CLI --r-home overrides everything
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
        return Ok(RSourceStatus::ExplicitPath { path: r_home });
    }

    // CLI --with-r-version overrides config (uses rig)
    if let Some(version) = cli_version {
        return setup_r_via_rig(version);
    }

    match r_source {
        RSource::Mode(RSourceMode::Auto) => {
            // Auto mode: try rig if available, otherwise use PATH
            if external::rig::rig_available() {
                match external::rig::resolve_version("default") {
                    Ok(resolved) => {
                        log::info!("Using rig default R version: {}", resolved.version);
                        // SAFETY: We're single-threaded at this point during startup
                        unsafe { std::env::set_var("R_HOME", &resolved.r_home) };
                        return Ok(RSourceStatus::Rig {
                            version: resolved.version,
                        });
                    }
                    Err(e) => {
                        log::debug!("Could not get rig default version: {}", e);
                        log::info!("Using R from PATH");
                        // Fall through to use system R from PATH
                    }
                }
            } else {
                log::info!("Using R from PATH (rig not available)");
            }
            Ok(RSourceStatus::Path)
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
                    Ok(RSourceStatus::Rig {
                        version: resolved.version,
                    })
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
            Ok(RSourceStatus::ExplicitPath { path: path.clone() })
        }
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
        Ok(resolved) => {
            log::info!(
                "Using R version {} from {}",
                resolved.version,
                resolved.r_home
            );
            // SAFETY: We're single-threaded at this point during startup
            unsafe { std::env::set_var("R_HOME", &resolved.r_home) };
            Ok(RSourceStatus::Rig {
                version: resolved.version,
            })
        }
        Err(e) => {
            anyhow::bail!("Failed to resolve R version '{}': {}", version_spec, e);
        }
    }
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
