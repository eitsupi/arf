//! Headless mode: R + IPC server without an interactive REPL.

use crate::app::config_load::{load_config_collecting_warnings, load_config_or_warn};
#[cfg(windows)]
use crate::app::r_profiles::source_r_profiles;
use crate::app::session_id::create_session_id;
use crate::app::setup::{RSourceOverrideState, RSourceResolutionReport, setup_r};
use crate::cli::RArgsBuilder;
use crate::config;
use crate::ipc;
use crate::ipc::session::SessionInfo;
use crate::pid_file::{
    absolute_pid_file_path, cleanup_ipc_pid_file, register_ipc_pid_file_atexit, write_pid_file,
};
use anyhow::{Context, Result};
use serde::Serialize;

/// JSON output for `arf headless --json`.
///
/// Contains session connection info and any warnings collected during startup.
/// All keys are always present in the JSON output; `r_version`, `log_file`,
/// and `history_session_id` may be `null`. `warnings` is an array that may be
/// empty.
#[derive(Debug, Serialize)]
struct HeadlessInfo {
    pid: u32,
    socket_path: String,
    r_version: Option<String>,
    cwd: String,
    started_at: String,
    log_file: Option<String>,
    history_session_id: Option<i64>,
    r_source_override: HeadlessRSourceOverride,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct HeadlessRSourceOverride {
    state: String,
    provider: Option<String>,
    file: Option<String>,
    key: Option<String>,
    requested_version: Option<String>,
    resolved_version: Option<String>,
}

impl HeadlessRSourceOverride {
    pub(crate) fn from_report(report: &RSourceResolutionReport) -> Self {
        let applied = report.override_state == RSourceOverrideState::Applied;
        Self {
            state: report.override_state.as_str().to_owned(),
            provider: applied.then(|| report.provider.clone()).flatten(),
            file: applied
                .then(|| report.file.as_ref().map(|path| path.display().to_string()))
                .flatten(),
            key: applied.then(|| report.key.clone()).flatten(),
            requested_version: applied.then(|| report.requested_version.clone()).flatten(),
            resolved_version: applied.then(|| report.resolved_version.clone()).flatten(),
        }
    }
}

impl HeadlessInfo {
    fn from_session(
        session: &SessionInfo,
        warnings: Vec<String>,
        resolution: &RSourceResolutionReport,
    ) -> Self {
        // Normalize empty/whitespace-only R version to None so JSON shows null
        let r_version = session
            .r_version
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(|s| s.to_string());

        Self {
            pid: session.pid,
            socket_path: session.socket_path.clone(),
            r_version,
            cwd: session.cwd.clone(),
            started_at: session.started_at.clone(),
            log_file: session.log_file.clone(),
            history_session_id: session.history_session_id,
            r_source_override: HeadlessRSourceOverride::from_report(resolution),
            warnings,
        }
    }
}

/// Run in headless mode: R + IPC server, no interactive REPL.
///
/// Initializes R, starts the IPC server, and enters a polling loop.
/// The loop processes IPC requests and R events until interrupted
/// by Ctrl+C or a shutdown signal.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_headless(
    config_path: Option<&std::path::PathBuf>,
    r_home: Option<&std::path::Path>,
    r_version: Option<&str>,
    r_args_builder: RArgsBuilder<'_>,
    bind: Option<&str>,
    pid_file: Option<&std::path::Path>,
    quiet: bool,
    json: bool,
    log_file: Option<&std::path::Path>,
    cli_history_dir: Option<&std::path::Path>,
    no_history: bool,
    no_r_source_overrides: bool,
) -> Result<()> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // --json implies --quiet: suppress status messages on stderr since
    // all relevant info is in the JSON output on stdout.
    let quiet = quiet || json;

    log::info!("Starting arf in headless mode");

    // Collect warnings for --json output instead of printing to stderr
    let mut warnings: Vec<String> = Vec::new();

    // Load config for r_source resolution
    let mut config = if json {
        load_config_collecting_warnings(config_path, &mut warnings)
    } else {
        load_config_or_warn(config_path)
    };

    // Set up R
    let resolution = setup_r(
        &config.startup.r_source,
        &config.experimental.r_source_overrides,
        None,
        r_home,
        r_version,
        no_r_source_overrides,
    )?;
    if json {
        warnings.extend(resolution.diagnostics.iter().cloned());
    } else {
        resolution.emit_diagnostics();
    }

    // Ensure LD_LIBRARY_PATH includes R library directory
    if let Err(e) = arf_libr::ensure_ld_library_path() {
        log::warn!("Could not set LD_LIBRARY_PATH: {}", e);
    }

    // Generate R initialization arguments
    let r_args = r_args_builder.build();
    let r_args_refs: Vec<&str> = r_args.iter().map(|s| s.as_str()).collect();

    // Initialize R
    unsafe {
        arf_libr::initialize_r_with_args(&r_args_refs).context("Failed to initialize R")?;
    }

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

    // Set up shutdown flag (shared between Ctrl+C handler and IPC shutdown method)
    let shutdown = Arc::new(AtomicBool::new(false));
    ipc::set_headless_shutdown(shutdown.clone());

    // Apply CLI history overrides (same logic as the REPL path in main())
    if no_history {
        config.history.disabled = true;
    } else if let Some(history_dir) = cli_history_dir {
        config.history.dir = Some(history_dir.to_path_buf());
    }

    // Initialize history for headless mode (same SQLite database as the REPL).
    // Only advertise history_session_id to IPC if the backend was actually opened.
    let session_id = create_session_id(&config);
    let mut session_id_raw = None;
    if let Some(sid) = session_id {
        let history_path = {
            let dir = config.history.dir.clone().or_else(config::history_dir);
            dir.map(|d| d.join("r.db"))
        };
        if let Some(path) = history_path {
            match reedline::SqliteBackedHistory::with_file(
                path.clone(),
                Some(sid),
                Some(chrono::Utc::now()),
            ) {
                Ok(history) => {
                    ipc::set_headless_history(history);
                    ipc::set_history_db_info(path.clone(), Some(sid));
                    session_id_raw = Some(i64::from(sid));
                    log::info!("Headless history enabled: {}", path.display());
                }
                Err(e) => {
                    log::warn!("Failed to open history database {}: {}", path.display(), e);
                }
            }
        }
    }

    // Start IPC server (with optional custom bind path)
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
    let session = ipc::start_server(bind, log_file_str, session_id_raw)
        .context("Failed to start IPC server")?;
    if !quiet {
        eprintln!("IPC server listening on: {}", session.socket_path);
    }

    // Write PID file if requested
    if let Some(pid_path) = pid_file {
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
    }

    // Set up signal handler for graceful shutdown.
    // With the "termination" feature, ctrlc also handles SIGTERM and SIGHUP,
    // enabling clean shutdown from systemd stop, docker stop, nohup hangup, etc.
    let shutdown_signal = shutdown.clone();
    if let Err(e) = ctrlc::set_handler(move || {
        shutdown_signal.store(true, Ordering::Release);
    }) {
        log::warn!("Could not set Ctrl+C handler: {}", e);
    }

    // Mark R as ready for IPC requests
    ipc::set_r_at_prompt(true);

    if json {
        // Output session info as JSON to stdout
        let output = HeadlessInfo::from_session(&session, warnings, &resolution);
        let is_tty = std::io::IsTerminal::is_terminal(&std::io::stdout());
        let json_str = if is_tty {
            serde_json::to_string_pretty(&output)
        } else {
            serde_json::to_string(&output)
        }
        .context("Failed to serialize session info")?;
        // Use writeln + flush instead of println to ensure the JSON is
        // delivered immediately when stdout is piped (non-TTY). This is the
        // readiness signal for CI scripts waiting on the output.
        use std::io::Write;
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{json_str}").context("Failed to write session info to stdout")?;
        stdout
            .flush()
            .context("Failed to flush session info to stdout")?;
    } else if !quiet {
        eprintln!("Headless mode ready. Press Ctrl+C to exit.");
    }

    // Main event loop
    while !shutdown.load(Ordering::Acquire) {
        // Process IPC requests
        let had_work = ipc::headless_poll_and_process();

        // Process R events (timers, background tasks, etc.)
        arf_libr::process_r_events();

        // Sleep to avoid busy loop — shorter if we had work (more may be coming)
        if had_work {
            std::thread::sleep(std::time::Duration::from_millis(1));
        } else {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn report(state: RSourceOverrideState) -> RSourceResolutionReport {
        RSourceResolutionReport {
            status: crate::config::RSourceStatus::Path,
            r_home: Some("/tmp/r-home".into()),
            provider: None,
            file: None,
            key: None,
            requested_version: None,
            resolved_version: None,
            diagnostics: Vec::new(),
            override_state: state,
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
            assert_eq!(value["state"], state.as_str());
            assert!(value.get("provider").is_some());
            assert!(value.get("file").is_some());
            assert!(value.get("key").is_some());
            assert!(value.get("requested_version").is_some());
            assert!(value.get("resolved_version").is_some());
        }
    }

    #[test]
    fn applied_r_source_override_json_contains_resolution_metadata() {
        let mut report = report(RSourceOverrideState::Applied);
        report.provider = Some("toml-key".to_string());
        report.file = Some("rproject.toml".into());
        report.key = Some("project.r_version".to_string());
        report.requested_version = Some("4.4".to_string());
        report.resolved_version = Some("4.4.2".to_string());

        let value = serde_json::to_value(HeadlessRSourceOverride::from_report(&report)).unwrap();
        assert_eq!(value["state"], "applied");
        assert_eq!(value["provider"], "toml-key");
        assert_eq!(value["file"], "rproject.toml");
        assert_eq!(value["key"], "project.r_version");
        assert_eq!(value["requested_version"], "4.4");
        assert_eq!(value["resolved_version"], "4.4.2");
    }
}
