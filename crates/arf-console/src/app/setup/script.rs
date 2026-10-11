//! Script execution mode.

use crate::app::config_load::{
    load_config_for_startup, report_diagnostics_on_setup_error, report_startup_diagnostics,
};
use crate::app::explicit_r_source::ExplicitRSourceRequest;
use crate::cli::Cli;
use crate::config::ReprexMode;
use crate::external::formatter;
use anyhow::{Context, Result};
use std::fs;

/// Run in script execution mode (non-interactive).
pub(crate) fn run_script(
    cli: &Cli,
    explicit_request: Option<&ExplicitRSourceRequest>,
) -> Result<()> {
    // Load configuration (from file or default)
    let (config, config_diagnostics) = load_config_for_startup(cli.r_source.config.as_ref());

    // Set up R based on r_source config (with optional CLI override)
    let (resolution, config_diagnostics) = report_diagnostics_on_setup_error(
        super::setup_r(
            &config.startup.r_source,
            &config.experimental.r_source_overrides,
            None,
            explicit_request,
            cli.r_source.no_r_source_overrides,
        ),
        config_diagnostics,
        report_startup_diagnostics,
    )?;
    let override_notice = super::overrides::script_override_notice(&resolution);

    // Prepare the platform loader path for the R library directory.
    #[cfg(unix)]
    let loader_result = arf_libr::ensure_ld_library_path_with_pre_exec(|| {
        super::forward_loader_target(&resolution)
    });
    #[cfg(not(unix))]
    let loader_result = arf_libr::ensure_ld_library_path();
    if let Err(e) = loader_result {
        log::warn!("Could not prepare the R library search path: {}", e);
    }
    #[cfg(unix)]
    super::clear_loader_target();

    report_startup_diagnostics(config_diagnostics);
    resolution.emit_diagnostics();
    if let Some(notice) = override_notice {
        eprintln!("{notice}");
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
    let code = normalize_line_endings(code);

    // Evaluate the code using the CLI override or configured reprex mode.
    let mut reprex_mode = cli.reprex.unwrap_or(config.startup.reprex);
    let formatter_selector = config.reprex.formatter;
    let formatter_backend = formatter::resolve_formatter(formatter_selector);
    if reprex_mode == ReprexMode::Format && formatter_backend.is_none() {
        if cli.reprex.is_some() {
            anyhow::bail!(
                "{}",
                formatter::unavailable_message(
                    formatter_selector,
                    formatter::FormatterUnavailableContext::ExplicitCli
                )
            );
        }
        eprintln!(
            "{}",
            formatter::unavailable_message(
                formatter_selector,
                formatter::FormatterUnavailableContext::ConfiguredMode
            )
        );
        reprex_mode = ReprexMode::On;
    }
    if reprex_mode != ReprexMode::Off {
        let code = if reprex_mode == ReprexMode::Format {
            formatter::format_code(
                formatter_backend.expect("format mode requires a resolved formatter"),
                &code,
            )?
        } else {
            code
        };
        // In reprex mode, echo source code before each result
        arf_harp::eval_string_reprex(&code, &config.reprex.comment)
    } else {
        // Normal script execution
        arf_harp::eval_string(&code)
    }
    // Report only the top-level message; its source chain repeats the same text.
    .map(|_| ())
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// Convert CRLF line endings to LF, as Rscript does when reading R code.
///
/// R's string parser rejects a CR before LF, while lone CR characters are left
/// unchanged so that they behave the same as in Rscript.
fn normalize_line_endings(code: String) -> String {
    if code.contains("\r\n") {
        code.replace("\r\n", "\n")
    } else {
        code
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_line_endings;

    #[test]
    fn normalize_line_endings_converts_only_crlf_pairs() {
        assert_eq!(
            normalize_line_endings("x <- \"a\r\nb\"\r\ny\r\n".to_string()),
            "x <- \"a\nb\"\ny\n"
        );
        assert_eq!(normalize_line_endings("a\rb\n".to_string()), "a\rb\n");
    }
}
