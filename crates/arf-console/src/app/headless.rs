//! Headless mode: R + IPC server without an interactive REPL.

use crate::app::config_load::{load_config_collecting_warnings, load_config_or_warn};
#[cfg(windows)]
use crate::app::r_profiles::source_r_profiles;
use crate::app::session_id::create_session_id;
use crate::app::setup::{RSourceOverrideState, RSourceResolutionReport, setup_r};
use crate::cli::RArgsBuilder;
use crate::config;
use crate::history::HistoryRuntime;
use crate::ipc;
use crate::ipc::session::{SessionInfo, SessionType};
use crate::output::write_json;
use crate::pid_file::{
    absolute_pid_file_path, cleanup_ipc_pid_file, register_ipc_pid_file_atexit, write_pid_file,
};
use anyhow::{Context, Result};
use serde::Serialize;

/// JSON output for `arf headless --json`.
///
/// Contains session connection info and any warnings collected during startup.
/// All keys are always present in the JSON output; `r_version`, `r_home`, `log_file`,
/// and `history_session_id` may be `null` only when history initialization is
/// unavailable. `warnings` is an array that may be empty.
#[derive(Debug, Serialize)]
struct HeadlessInfo {
    pid: u32,
    socket_path: String,
    r_version: Option<String>,
    r_home: Option<String>,
    cwd: String,
    started_at: String,
    log_file: Option<String>,
    history_session_id: Option<i64>,
    history_runtime: HeadlessHistoryRuntime,
    r_source_override: HeadlessRSourceOverride,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct HeadlessHistoryRuntime {
    state: String,
    path: Option<String>,
    reason: Option<String>,
}

impl HeadlessHistoryRuntime {
    fn from_runtime(runtime: &HistoryRuntime) -> Self {
        Self {
            state: runtime.state_name().to_string(),
            path: runtime
                .requested_path()
                .map(|path| path.display().to_string()),
            reason: runtime.reason_name().map(str::to_string),
        }
    }
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
        history_runtime: &HistoryRuntime,
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
            r_home: session.r_home.clone(),
            cwd: session.cwd.clone(),
            started_at: session.started_at.clone(),
            log_file: session.log_file.clone(),
            history_session_id: session.history_session_id,
            history_runtime: HeadlessHistoryRuntime::from_runtime(history_runtime),
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
    ipc_eval_allow_function: &[String],
    ipc_eval_unrestricted: bool,
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
        warnings.extend(
            resolution
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.clone()),
        );
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

    // Set up shutdown flag (shared between Ctrl+C handler and IPC shutdown method)
    let shutdown = Arc::new(AtomicBool::new(false));
    ipc::set_headless_shutdown(shutdown.clone());

    // Apply CLI history overrides (same logic as the REPL path in main())
    if no_history {
        config.history.mode = crate::config::HistoryMode::Volatile;
    } else if let Some(history_dir) = cli_history_dir {
        config.history.mode = crate::config::HistoryMode::Persistent {
            dir: Some(history_dir.to_path_buf()),
        };
    }

    let mut eval_allowlist = config.ipc.eval.allowed_functions.clone();
    eval_allowlist.extend(ipc_eval_allow_function.iter().cloned());
    ipc::policy::set_policy(eval_allowlist, ipc_eval_unrestricted);

    // Initialize the single history owner for headless mode. Volatile history
    // is still queryable through IPC during this process but never touches disk.
    let session_id = create_session_id(&config);
    let history_path =
        config::history_dir_for_mode(&config.history.mode).map(|dir| dir.join("r.db"));
    let history_runtime = HistoryRuntime::initialize(
        &config.history.mode,
        history_path,
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
    let session_id_raw = history_runtime
        .store()
        .and_then(|store| store.session())
        .map(i64::from);

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
    let r_home_str = r_home.map(|path| path.display().to_string());
    let session = ipc::start_server(
        bind,
        r_home_str,
        log_file_str,
        session_id_raw,
        SessionType::Headless,
    )
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
        let output = HeadlessInfo::from_session(&session, &history_runtime, warnings, &resolution);
        // Use writeln + flush instead of println to ensure the JSON is
        // delivered immediately when stdout is piped (non-TTY). This is the
        // readiness signal for CI scripts waiting on the output.
        use std::io::Write;
        let pretty = std::io::IsTerminal::is_terminal(&std::io::stdout());
        let mut stdout = std::io::stdout().lock();
        write_json(&mut stdout, &output, pretty)
            .context("Failed to write session info to stdout")?;
        writeln!(stdout).context("Failed to write session info newline to stdout")?;
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
    fn headless_json_includes_r_home_from_session() {
        let history_runtime =
            HistoryRuntime::initialize(&crate::config::HistoryMode::Volatile, None, None, None);
        let mut session = SessionInfo {
            pid: 12345,
            socket_path: "/tmp/arf.sock".to_string(),
            r_version: Some("4.4.1".to_string()),
            r_home: Some("/opt/R/4.4.1/lib/R".to_string()),
            cwd: "/tmp".to_string(),
            started_at: "2026-01-01T00:00:00+00:00".to_string(),
            session_type: Some(SessionType::Headless),
            log_file: None,
            history_session_id: None,
        };

        let output = HeadlessInfo::from_session(
            &session,
            &history_runtime,
            Vec::new(),
            &report(RSourceOverrideState::NotConfigured),
        );
        let json = serde_json::to_value(output).unwrap();
        assert_eq!(json["r_home"], "/opt/R/4.4.1/lib/R");
        assert_eq!(json["history_runtime"]["state"], "volatile");
        assert_eq!(json["history_runtime"]["reason"], "configured");

        session.r_home = None;
        let output = HeadlessInfo::from_session(
            &session,
            &history_runtime,
            Vec::new(),
            &report(RSourceOverrideState::NotConfigured),
        );
        let json = serde_json::to_value(output).unwrap();
        assert!(json["r_home"].is_null());
    }

    #[test]
    fn headless_history_runtime_uses_stable_reason_and_separate_path() {
        use crate::history::{
            HistoryHandle, HistorySaveReceipt, HistoryStore, VolatileHistoryReason,
        };

        let store = HistoryStore::in_memory(None, None).unwrap();
        let handle = || HistoryHandle {
            store: store.clone(),
            receipt: HistorySaveReceipt::new(),
        };
        let configured = HistoryRuntime::Volatile {
            handle: handle(),
            reason: VolatileHistoryReason::Configured,
        };
        let fallback = HistoryRuntime::Volatile {
            handle: handle(),
            reason: VolatileHistoryReason::Fallback {
                requested_path: Some("/tmp/history.db".into()),
            },
        };
        let fallback_without_path = HistoryRuntime::Volatile {
            handle: handle(),
            reason: VolatileHistoryReason::Fallback {
                requested_path: None,
            },
        };
        let unavailable = HistoryRuntime::Unavailable {
            requested_path: Some("/tmp/history.db".into()),
        };
        let persistent_dir = tempfile::tempdir().unwrap();
        let persistent_path = persistent_dir.path().join("history.db");
        let persistent = HistoryRuntime::Persistent(HistoryHandle {
            store: HistoryStore::open(persistent_path.clone(), None, None).unwrap(),
            receipt: HistorySaveReceipt::new(),
        });

        assert!(configured.startup_warning().is_none());
        assert!(fallback.startup_warning().is_some());
        assert!(unavailable.startup_warning().is_some());
        let persistent_json =
            serde_json::to_value(HeadlessHistoryRuntime::from_runtime(&persistent)).unwrap();
        assert_eq!(persistent_json["state"], "persistent");
        assert!(persistent_json["reason"].is_null());
        assert_eq!(
            persistent_json["path"],
            persistent_path.display().to_string()
        );

        let configured_json =
            serde_json::to_value(HeadlessHistoryRuntime::from_runtime(&configured)).unwrap();
        assert_eq!(configured_json["state"], "volatile");
        assert_eq!(configured_json["reason"], "configured");
        assert!(configured_json["path"].is_null());

        let fallback_json =
            serde_json::to_value(HeadlessHistoryRuntime::from_runtime(&fallback)).unwrap();
        assert_eq!(fallback_json["reason"], "fallback");
        assert_eq!(fallback_json["path"], "/tmp/history.db");
        let no_path_json =
            serde_json::to_value(HeadlessHistoryRuntime::from_runtime(&fallback_without_path))
                .unwrap();
        assert_eq!(no_path_json["reason"], "fallback");
        assert!(no_path_json["path"].is_null());

        let unavailable_json =
            serde_json::to_value(HeadlessHistoryRuntime::from_runtime(&unavailable)).unwrap();
        assert_eq!(unavailable_json["state"], "unavailable");
        assert_eq!(unavailable_json["reason"], "initialization_failed");
        assert_eq!(unavailable_json["path"], "/tmp/history.db");
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
