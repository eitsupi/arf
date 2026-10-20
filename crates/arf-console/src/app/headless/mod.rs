//! Headless mode: R + IPC server without an interactive REPL.

mod info;
#[cfg(test)]
mod tests;

use crate::app::config_load::{
    StartupDiagnostic, load_config_collecting_warnings, load_config_for_startup,
    report_diagnostics_on_setup_error, report_startup_diagnostics,
};
use crate::app::explicit_r_source::ExplicitRSourceRequest;
#[cfg(windows)]
use crate::app::r_profiles::source_r_profiles;
use crate::app::session_id::create_session_id;
use crate::app::setup::{RSourceResolutionReport, setup_r};
use crate::cli::RArgsBuilder;
use crate::config::{self, Config, ConfigLoadProvenance};
use crate::history::HistoryRuntime;
use crate::ipc;
use crate::ipc::session::{SessionInfo, SessionType};
use crate::output::write_json;
use crate::pid_file::{
    absolute_pid_file_path, cleanup_ipc_pid_file, register_ipc_pid_file_atexit, write_pid_file,
};
use anyhow::{Context, Result};
use info::HeadlessInfo;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Run in headless mode: R + IPC server, without a terminal UI.
///
/// Initializes R and starts the IPC server. R's mainloop owns request input
/// until interrupted by Ctrl+C or a shutdown signal.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_headless(
    config_path: Option<&std::path::PathBuf>,
    explicit_request: Option<&ExplicitRSourceRequest>,
    r_args_builder: RArgsBuilder<'_>,
    bind: Option<&str>,
    pid_file: Option<&std::path::Path>,
    quiet: bool,
    json: bool,
    log_file: Option<&std::path::Path>,
    cli_history_dir: Option<&std::path::Path>,
    no_history: bool,
    no_r_source_overrides: bool,
    ipc_eval_allow_function: &[String],
    ipc_eval_unrestricted: bool,
) -> Result<()> {
    if let Some(path) = pid_file {
        // Resolve before R startup can change the working directory.
        crate::pid_file::set_initial_pid_file_path(path);
    }
    let launch_cwd = std::env::current_dir().ok();

    // --json implies --quiet: suppress status messages on stderr since
    // all relevant info is in the JSON output on stdout.
    let quiet = quiet || json;

    log::info!("Starting arf in headless mode");

    // Collect warnings for --json output instead of printing to stderr
    let mut warnings: Vec<String> = Vec::new();

    // Load config for r_source resolution
    let (mut config, config_diagnostics, loaded_config) =
        load_headless_config(config_path, json, &mut warnings);

    // Set up R
    let (resolution, config_diagnostics) = resolve_r_source(
        &config,
        explicit_request,
        no_r_source_overrides,
        json,
        config_diagnostics,
        &mut warnings,
    )?;

    crate::app::setup::prepare_loader_path(&resolution);

    if !json {
        report_startup_diagnostics(config_diagnostics);
        resolution.emit_diagnostics();
    }

    let r_home = initialize_r(r_args_builder)?;

    // Set up shutdown flag (shared between Ctrl+C handler and IPC shutdown method)
    let shutdown = Arc::new(AtomicBool::new(false));
    ipc::set_headless_shutdown(shutdown.clone());

    // Apply CLI history overrides (same logic as the REPL path in main())
    config.history.mode =
        config::history_mode_with_overrides(&config.history.mode, cli_history_dir, no_history);

    install_ipc_eval_policy(&config, ipc_eval_allow_function, ipc_eval_unrestricted);

    let (history_runtime, history_source) = initialize_history(&config, json, &mut warnings);

    let session = start_ipc_server(bind, r_home, log_file, &history_runtime, quiet)?;

    // Write PID file if requested
    if let Some(pid_path) = pid_file {
        write_headless_pid_file(pid_path)?;
    }

    install_shutdown_handler(&shutdown);

    // Mark R as ready for IPC requests
    ipc::set_r_at_prompt(true);

    if json {
        // Output session info as JSON to stdout
        let output = HeadlessInfo::from_session(
            &session,
            &history_runtime,
            history_source,
            warnings,
            &resolution,
            launch_cwd
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new(".")),
            loaded_config.as_ref(),
        );
        write_session_info(&output)?;
    } else if !quiet {
        eprintln!("Headless mode ready. Press Ctrl+C to exit.");
    }

    arf_libr::set_headless_read_console_callback(ipc::headless_read_console);
    unsafe { arf_libr::run_r_mainloop() };

    if !quiet {
        eprintln!("\nShutting down...");
    }
    ipc::stop_server();

    // Clean up PID file.
    if let Some(pid_path) = pid_file {
        cleanup_ipc_pid_file(pid_path);
    }

    Ok(())
}

/// Load the config used for R source resolution.
///
/// With `--json`, load warnings are appended to `warnings` instead of being
/// returned as diagnostics, and the load provenance is reported.
fn load_headless_config(
    config_path: Option<&std::path::PathBuf>,
    json: bool,
    warnings: &mut Vec<String>,
) -> (Config, Vec<StartupDiagnostic>, Option<ConfigLoadProvenance>) {
    if json {
        let (config, provenance) = load_config_collecting_warnings(config_path, warnings);
        (config, Vec::new(), provenance)
    } else {
        let (config, diagnostics) = load_config_for_startup(config_path);
        (config, diagnostics, None)
    }
}

/// Resolve the R installation to use.
///
/// With `--json`, resolution diagnostics are appended to `warnings`; otherwise
/// pending config diagnostics are reported before a setup error is returned.
fn resolve_r_source(
    config: &Config,
    explicit_request: Option<&ExplicitRSourceRequest>,
    no_r_source_overrides: bool,
    json: bool,
    config_diagnostics: Vec<StartupDiagnostic>,
    warnings: &mut Vec<String>,
) -> Result<(RSourceResolutionReport, Vec<StartupDiagnostic>)> {
    let setup_result = setup_r(
        &config.startup.r_source,
        &config.experimental.r_source_overrides,
        None,
        explicit_request,
        no_r_source_overrides,
    );
    let (resolution, config_diagnostics) = if json {
        (setup_result?, config_diagnostics)
    } else {
        report_diagnostics_on_setup_error(
            setup_result,
            config_diagnostics,
            report_startup_diagnostics,
        )?
    };
    if json {
        warnings.extend(
            resolution
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone()),
        );
    }
    Ok((resolution, config_diagnostics))
}

/// Initialize embedded R and configure it for headless operation.
///
/// Returns the runtime `R_HOME`, if it could be determined.
fn initialize_r(r_args_builder: RArgsBuilder<'_>) -> Result<Option<std::path::PathBuf>> {
    // Generate R initialization arguments
    let r_args = r_args_builder.build();
    let r_args_refs: Vec<&str> = r_args.iter().map(|s| s.as_str()).collect();

    // Initialize R. Note the directory beforehand: profiles run inside
    // initialization on Unix and may call setwd(), which would move the base a
    // relative R_HOME has to be resolved against.
    let pre_init_dir = std::env::current_dir().ok();
    unsafe {
        arf_libr::initialize_r_with_args(&r_args_refs).context("Failed to initialize R")?;
    }
    let r_home = crate::capture_runtime_r_home(pre_init_dir.as_deref());

    // Source R profile files (Windows only)
    #[cfg(windows)]
    source_r_profiles(&r_args);

    // Configure R options for headless operation:
    // - Redirect pager output (help, file.show) to stdout so it gets captured
    //   by evaluate_with_capture instead of spawning an interactive pager (less)
    // - Force plain-text help (`options(help_type = "text")`) so help output
    //   is printable/capturable instead of opening HTML or other rich viewers
    // - Disable interactive browsers (`options(browser = ...)`) so R does not
    //   attempt to launch a GUI/web browser in headless environments
    // - Set default graphics device to file-based (png/pdf) instead of X11
    //   to avoid DISPLAY-related errors or hangs in headless environments
    configure_headless_r_options()?;

    Ok(r_home)
}

/// Install the IPC eval policy from the config plus CLI overrides.
fn install_ipc_eval_policy(
    config: &Config,
    ipc_eval_allow_function: &[String],
    ipc_eval_unrestricted: bool,
) {
    let mut eval_allowlist = config.ipc.eval.allowed_functions.clone();
    eval_allowlist.extend(ipc_eval_allow_function.iter().cloned());
    ipc::policy::set_policy(eval_allowlist, ipc_eval_unrestricted);
}

/// Initialize the single history owner for headless mode. Volatile history
/// is still queryable through IPC during this process but never touches disk.
fn initialize_history(
    config: &Config,
    json: bool,
    warnings: &mut Vec<String>,
) -> (HistoryRuntime, config::HistoryLocationSource) {
    let session_id = create_session_id(config);
    let history_location = config::resolved_history_location(&config.history.mode);
    if let Some(warning) = history_location.startup_warning() {
        log::warn!("Headless history: {warning}");
        if json {
            warnings.push(warning);
        } else {
            eprintln!("Warning: {warning}");
        }
    }
    let history_runtime = HistoryRuntime::initialize_for_location(
        &config.history.mode,
        &history_location,
        "r.db",
        crate::history::artifact::HistoryKind::R,
        session_id,
        Some(chrono::Utc::now()),
    );
    if let Some(history) = history_runtime.store() {
        ipc::set_headless_history(history);
        log::info!("Headless history runtime: {}", history_runtime.state_name());
        if let Some(warning) = history_runtime.startup_warning() {
            log::warn!("Headless history: {warning}");
            if json {
                warnings.push(warning);
            } else {
                eprintln!("Warning: Headless history: {warning}");
            }
        }
    } else {
        let warning = history_runtime
            .startup_warning()
            .unwrap_or_else(|| "history unavailable".to_string());
        log::warn!("Headless history unavailable: {warning}");
        if json {
            warnings.push(warning);
        } else {
            eprintln!("Warning: Headless history unavailable: {warning}");
        }
    }
    (history_runtime, history_location.source())
}

/// Start the IPC server (with optional custom bind path).
fn start_ipc_server(
    bind: Option<&str>,
    r_home: Option<std::path::PathBuf>,
    log_file: Option<&std::path::Path>,
    history_runtime: &HistoryRuntime,
    quiet: bool,
) -> Result<SessionInfo> {
    let session_id_raw = history_runtime
        .store()
        .and_then(|store| store.session())
        .map(i64::from);

    let log_file_str = log_file.map(|p| {
        // Convert to absolute path so IPC clients can locate the file
        // regardless of their own working directory. Use std::path::absolute
        // instead of canonicalize because the file may not exist yet at this
        // point (the logger creates it).
        std::path::absolute(p)
            .unwrap_or_else(|_| p.to_path_buf())
            .display()
            .to_string()
    });
    let r_home_str = r_home.map(|path| path.display().to_string());
    let session = ipc::start_server(
        bind,
        r_home_str,
        ipc::read_r_version(),
        log_file_str,
        session_id_raw,
        SessionType::Headless,
    )
    .context("Failed to start IPC server")?;
    if !quiet {
        eprintln!("IPC server listening on: {}", session.socket_path);
    }
    Ok(session)
}

/// Write the PID file, stopping the IPC server if that fails.
fn write_headless_pid_file(pid_path: &std::path::Path) -> Result<()> {
    let pid_path = absolute_pid_file_path(pid_path);
    if let Err(e) = write_pid_file(&pid_path) {
        // Do not attempt to remove the PID file here: write_pid_file uses
        // create_new and may have failed before creating it (e.g. AlreadyExists),
        // so pid_path may refer to a pre-existing user-managed file.

        // Stop IPC server to avoid leaving a stale socket/session behind.
        ipc::stop_server();

        return Err(e);
    }
    register_ipc_pid_file_atexit(&pid_path);
    Ok(())
}

/// Set up signal handler for graceful shutdown.
///
/// With the "termination" feature, ctrlc also handles SIGTERM and SIGHUP,
/// enabling clean shutdown from systemd stop, docker stop, nohup hangup, etc.
fn install_shutdown_handler(shutdown: &Arc<AtomicBool>) {
    let shutdown_signal = shutdown.clone();
    if let Err(e) = ctrlc::set_handler(move || {
        shutdown_signal.store(true, Ordering::Release);
    }) {
        log::warn!("Could not set Ctrl+C handler: {}", e);
    }
}

/// Write the session info JSON to stdout.
fn write_session_info(output: &HeadlessInfo) -> Result<()> {
    // Use writeln + flush instead of println to ensure the JSON is
    // delivered immediately when stdout is piped (non-TTY). This is the
    // readiness signal for CI scripts waiting on the output.
    use std::io::Write;
    let pretty = std::io::IsTerminal::is_terminal(&std::io::stdout());
    let mut stdout = std::io::stdout().lock();
    write_json(&mut stdout, output, pretty).context("Failed to write session info to stdout")?;
    writeln!(stdout).context("Failed to write session info newline to stdout")?;
    stdout
        .flush()
        .context("Failed to flush session info to stdout")?;
    Ok(())
}

/// Configure R options for headless mode.
///
/// Sets up pager redirection and graphics device defaults so that commands
/// like `?mean` or `plot(1:10)` don't spawn interactive programs (less, X11)
/// that would block or corrupt the headless server.
///
/// The approach is based on [mcp-repl](https://github.com/t-kalinowski/mcp-repl)
/// (Apache-2.0), which uses the same pattern of custom pager and device
/// functions for non-interactive R sessions.
fn configure_headless_r_options() -> Result<()> {
    let code = r#"
local({
    # Force text-based help output (no HTML browser)
    options(help_type = "text")

    # Custom pager: dump file contents to stdout instead of spawning less/more.
    # Output goes through WriteConsoleEx callback, so evaluate_with_capture
    # picks it up automatically.
    .arf_headless_pager <- function(files, header = NULL, title = NULL,
                                    delete.file = FALSE, ...) {
        files <- as.character(files)
        if (length(files) == 0L) return(invisible(NULL))

        if (!is.null(title) && length(title) >= 1L && nzchar(title[[1L]])) {
            cat(title[[1L]], "\n", sep = "")
        }

        for (i in seq_along(files)) {
            path <- files[[i]]
            if (!nzchar(path) || !file.exists(path)) next

            if (!is.null(header) && length(header) >= i && nzchar(header[[i]])) {
                cat(header[[i]], "\n", sep = "")
            }

            tryCatch({
                lines <- readLines(path, warn = FALSE)
                cat(lines, sep = "\n")
                if (length(lines) > 0L) cat("\n")
            }, error = function(e) NULL)

            if (isTRUE(delete.file)) unlink(path, force = TRUE)
        }
        invisible(NULL)
    }

    options(pager = .arf_headless_pager)
    options(help.pager = .arf_headless_pager)

    # Suppress browseURL() — just print the URL
    options(browser = function(url, ...) { cat(url, "\n"); invisible(0L) })

    # Default graphics device: png with pdf fallback.
    # Prevents X11/quartz from being opened in headless environments.
    .arf_headless_device <- function(...) {
        # Ignore ... to avoid unit mismatch: dev.new() passes width/height
        # in inches, but png() interprets them as pixels by default.
        # Use sensible defaults; Stage 2 can add proper argument handling.
        path <- tempfile("arf-headless-plot-", fileext = ".png")
        ok <- FALSE
        tryCatch({
            grDevices::png(filename = path)
            ok <- TRUE
        }, error = function(e) NULL)

        if (!ok) {
            path <- tempfile("arf-headless-plot-", fileext = ".pdf")
            grDevices::pdf(file = path)
        }

        # Enable display list recording for potential future plot retrieval
        try(grDevices::dev.control(displaylist = "enable"), silent = TRUE)
        invisible(NULL)
    }

    options(device = .arf_headless_device)
})
"#;

    arf_harp::eval_string_in_base(code)
        .context("Failed to configure headless R options (pager, browser, graphics device)")?;
    log::info!("Headless R options configured (pager, browser, graphics device)");
    Ok(())
}
