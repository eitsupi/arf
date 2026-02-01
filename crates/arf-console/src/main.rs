//! arf: A cross-platform R console written in Rust.

mod cli;
mod completion;
mod config;
mod editor;
mod external;
mod fuzzy;
mod highlighter;
mod history;
mod pager;
pub(crate) mod r_parser;
mod repl;
mod traps;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, Commands, ConfigAction, HistoryAction};
use config::{
    RSource, RSourceMode, RSourceStatus, config_file_path, ensure_directories, init_config,
    load_config, load_config_from_path,
};
use repl::Repl;
use std::fs;
#[cfg(windows)]
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {:#}", e);
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    env_logger::init();

    // Install signal handlers for fatal signals (SIGSEGV, SIGILL, SIGBUS).
    // This prevents the process from hanging when R encounters a segmentation fault.
    traps::register_trap_handlers();

    // Parse command-line arguments
    let cli = Cli::parse();

    // Handle subcommands first
    match &cli.command {
        Some(Commands::Completions { shell }) => {
            Cli::print_completions(*shell);
            return Ok(());
        }
        Some(Commands::Config { action }) => {
            return handle_config_command(action);
        }
        Some(Commands::History { action }) => {
            return handle_history_command(action);
        }
        None => {}
    }

    // Check if we're in script execution mode
    let script_mode = cli.eval.is_some() || cli.script.is_some();

    if script_mode {
        // Script execution mode - no REPL, just run code and exit
        return run_script(&cli);
    }

    log::info!("Starting arf");

    // Ensure XDG directories exist
    ensure_directories()?;

    // Load configuration (from file or default)
    // Track the config path for :info command display
    let (mut config, config_path) = if let Some(path) = &cli.config {
        (load_config_from_path(path), Some(path.clone()))
    } else {
        // Use default XDG location
        let default_path = config_file_path();
        (load_config(), default_path)
    };
    log::debug!("Loaded config: {:?}", config);

    // Apply CLI overrides
    if cli.reprex {
        config.reprex.enabled = true;
    }
    if cli.auto_format {
        if !external::formatter::is_formatter_available() {
            anyhow::bail!(
                "Cannot enable auto-format: Air CLI ('air' command) not found in PATH.\n\
                 Install Air CLI from https://github.com/posit-dev/air"
            );
        }
        config.reprex.autoformat = true;
    }
    if cli.no_banner {
        config.startup.show_banner = false;
    }
    if cli.no_auto_match {
        config.editor.auto_match = false;
    }
    if cli.no_completion {
        config.completion.enabled = false;
    }

    // History configuration: CLI flag overrides default XDG location
    if cli.no_history {
        config.history.disabled = true;
    } else if let Some(history_dir) = &cli.history_dir {
        config.history.dir = Some(history_dir.clone());
    }

    // Warn if auto-format is enabled (via config) but Air CLI is not available
    if config.reprex.autoformat
        && !cli.auto_format
        && !external::formatter::is_formatter_available()
    {
        eprintln!(
            "Warning: Auto-format is enabled in config but Air CLI ('air' command) not found in PATH."
        );
        eprintln!(
            "         Auto-format has been disabled. Install Air CLI from https://github.com/posit-dev/air"
        );
        config.reprex.autoformat = false;
    }

    // Set up R based on r_source config (with optional CLI override)
    let r_source_status = setup_r(
        &config.startup.r_source,
        cli.r_home.as_deref(),
        cli.r_version.as_deref(),
    )?;
    log::debug!("R source status: {:?}", r_source_status);

    // Ensure LD_LIBRARY_PATH includes R library directory.
    // This may re-exec the current process if the path needs updating.
    if let Err(e) = arf_libr::ensure_ld_library_path() {
        log::warn!("Could not set LD_LIBRARY_PATH: {}", e);
    }

    // Generate R initialization arguments from CLI flags
    let r_args = cli.r_args();
    let r_args_refs: Vec<&str> = r_args.iter().map(|s| s.as_str()).collect();
    log::debug!("R args: {:?}", r_args);

    // Initialize R with CLI-specified flags
    log::info!("Initializing R...");
    #[allow(unused_variables)]
    let r_initialized = unsafe {
        match arf_libr::initialize_r_with_args(&r_args_refs) {
            Ok(()) => {
                log::info!("R initialized successfully");
                true
            }
            Err(e) => {
                eprintln!("Warning: Failed to initialize R: {}", e);
                eprintln!("R evaluation will not be available.");
                eprintln!("Make sure R is installed and R_HOME is set correctly.\n");
                false
            }
        }
    };

    // Source R profile files after R initialization (Windows only)
    // On Windows, R's built-in profile loading is disabled during initialization
    // (load_init_file = R_FALSE in arf-libr/src/sys.rs), so we must manually
    // source .Rprofile files here. On Unix, R handles this automatically.
    #[cfg(windows)]
    if r_initialized {
        source_r_profiles(&r_args);
    }

    // Create and run the REPL
    let mut repl = Repl::new(config, config_path, r_source_status)?;
    repl.run()?;

    Ok(())
}

/// Handle config subcommands.
fn handle_config_command(action: &ConfigAction) -> Result<()> {
    match action {
        ConfigAction::Init { force } => {
            let path = init_config(*force)?;
            println!("Configuration file created at: {}", path.display());
            Ok(())
        }
    }
}

fn handle_history_command(action: &HistoryAction) -> Result<()> {
    match action {
        HistoryAction::Schema => {
            pager::history_schema::print_schema().context("Failed to display history schema")
        }
    }
}

/// Run in script execution mode (non-interactive).
fn run_script(cli: &Cli) -> Result<()> {
    // Load configuration (from file or default)
    let config = if let Some(config_path) = &cli.config {
        load_config_from_path(config_path)
    } else {
        load_config()
    };

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

    // Get the code to execute
    let code = if let Some(eval_code) = &cli.eval {
        eval_code.clone()
    } else if let Some(script_path) = &cli.script {
        fs::read_to_string(script_path)
            .with_context(|| format!("Failed to read script file: {}", script_path.display()))?
    } else {
        // Should not happen - we checked script_mode earlier
        return Ok(());
    };

    // Evaluate the code - use reprex mode if enabled (CLI or config)
    let reprex_enabled = cli.reprex || config.reprex.enabled;
    if reprex_enabled {
        // In reprex mode, echo source code before each result
        match arf_harp::eval_string_reprex(&code, &config.reprex.comment) {
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
fn setup_r(
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
fn source_r_profiles(r_args: &[String]) {
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
}
