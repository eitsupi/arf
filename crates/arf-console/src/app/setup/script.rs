//! Script execution mode.

use crate::app::config_load::load_config_or_warn;
use crate::cli::Cli;
use crate::config::ReprexMode;
use crate::external::formatter;
use anyhow::{Context, Result};
use std::fs;

/// Run in script execution mode (non-interactive).
pub(crate) fn run_script(cli: &Cli) -> Result<()> {
    // Load configuration (from file or default)
    let config = load_config_or_warn(cli.r_source.config.as_ref());

    // Set up R based on r_source config (with optional CLI override)
    let resolution = super::setup_r(
        &config.startup.r_source,
        &config.experimental.r_source_overrides,
        None,
        cli.r_source.r_home.as_deref(),
        cli.r_source.r_version.as_deref(),
        cli.r_source.no_r_source_overrides,
    )?;
    resolution.emit_diagnostics();
    if let Some(notice) = super::overrides::script_override_notice(&resolution) {
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
    crate::app::r_profiles::source_r_profiles(&r_args);

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

    // Evaluate the code using the CLI override or configured reprex mode.
    let mut reprex_mode = cli.reprex.unwrap_or(config.startup.reprex);
    if reprex_mode == ReprexMode::Format
        && !formatter::is_formatter_available(config.reprex.formatter)
    {
        if cli.reprex.is_some() {
            anyhow::bail!("Cannot use --reprex=format: Air CLI ('air' command) not found in PATH.");
        }
        eprintln!(
            "Warning: Reprex format mode is configured but Air CLI ('air' command) was not found; using reprex on mode."
        );
        reprex_mode = ReprexMode::On;
    }
    if reprex_mode != ReprexMode::Off {
        let code = if reprex_mode == ReprexMode::Format {
            formatter::format_code(config.reprex.formatter, &code)
        } else {
            code
        };
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
