//! arf: A cross-platform R console written in Rust.

mod app;
mod cli;
mod completion;
mod config;
mod console_mode;
mod editor;
mod external;
mod fuzzy;
mod highlighter;
mod history;
mod ipc;
mod logging;
mod output;
mod pager;
mod pid_file;
pub(crate) mod r_parser;
mod repl;
pub mod rversion;
mod traps;

#[cfg(test)]
mod test_utils;

use anyhow::Result;
pub(crate) use app::arguments::normalized_args;
use app::arguments::{absolute_ipc_bind_path, initialize_normalized_args};
use app::commands::{handle_config_command, handle_history_command, handle_ipc_command};
use app::config_load::{
    StartupDiagnostic, load_config_with_fallback, report_diagnostics_on_setup_error,
    report_startup_diagnostics,
};
use app::headless::run_headless;
use app::option_scope::{r_source_origin, validate_top_level_scope};
#[cfg(windows)]
use app::r_profiles::source_r_profiles;
use app::resolve::{ResolveCommandError, print_error, run_resolve};
use app::session_id::create_session_id;
use app::setup::{run_script, setup_r};
use app::startup_env::capture_startup_env;
pub(crate) use app::startup_env::{
    STARTUP_ENV_CARRIER, capture_runtime_r_home, startup_env_carrier, startup_env_value,
};
use clap::{CommandFactory, FromArgMatches};
use cli::{Cli, Commands, RArgsBuilder, RCommand};
use config::{ReprexMode, ensure_directories};
use logging::init_logger;
use pid_file::{
    absolute_pid_file_path, cleanup_ipc_pid_file, register_ipc_pid_file_atexit, write_pid_file,
};
use repl::Repl;
use std::ffi::OsStr;
#[cfg(not(unix))]
use std::ffi::OsString;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            if let Some(resolve_error) = e.downcast_ref::<ResolveCommandError>() {
                print_error(resolve_error);
                return ExitCode::from(resolve_error.exit_code());
            }
            eprintln!("Error: {:#}", e);
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    capture_startup_env();

    // Parse command-line arguments first, then initialize the logger exactly
    // once based on the parsed command. This avoids the fragile pre-parse
    // detection that could miss global options before the subcommand.
    let command = Cli::command();
    let matches = command.clone().get_matches();
    validate_top_level_scope(&command, &matches);
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());
    if let Some(path) = cli.ipc_pid_file.as_deref() {
        // Resolve this before R initialization: profiles may change cwd, but
        // a relative PID path must continue to identify its initial file.
        pid_file::set_initial_pid_file_path(path);
        #[cfg(unix)]
        pid_file::authorize_inherited_pid_fd(&pid_file::absolute_pid_file_path(path));
    }
    let bind_path = cli.ipc_bind.as_deref().map(|path| {
        #[cfg(unix)]
        {
            absolute_ipc_bind_path(OsStr::new(path))
        }
        #[cfg(not(unix))]
        {
            OsString::from(path)
        }
    });
    let pid_path = cli
        .ipc_pid_file
        .as_deref()
        .map(|path| pid_file::absolute_pid_file_path(path).into_os_string());
    initialize_normalized_args(
        std::env::args_os().skip(1).collect(),
        bind_path.as_deref(),
        pid_path.as_deref(),
    );
    let no_r_auto_discovery = match &cli.command {
        Some(Commands::Headless(args)) => args.r_source.no_r_auto_discovery,
        Some(Commands::R(args)) => {
            let RCommand::Resolve(resolve_args) = &args.command;
            resolve_args.r_source.no_r_auto_discovery
        }
        _ => cli.r_source.no_r_auto_discovery,
    };
    arf_libr::set_r_auto_discovery_disabled(no_r_auto_discovery);

    // Reject combinations of -f/--file or -e/--eval with a subcommand.
    // clap cannot enforce this via conflicts_with because subcommand fields are
    // not referenceable as argument IDs in the derive API.
    if (cli.eval.is_some() || cli.file.is_some()) && cli.command.is_some() {
        let flag = if cli.eval.is_some() {
            "--eval"
        } else {
            "--file"
        };
        let subcommand = match &cli.command {
            Some(Commands::Completions(_)) => "completions",
            Some(Commands::Config(_)) => "config",
            Some(Commands::History(_)) => "history",
            Some(Commands::Ipc(_)) => "ipc",
            Some(Commands::Headless(_)) => "headless",
            Some(Commands::R(_)) => "r",
            None => unreachable!(),
        };
        Cli::command()
            .error(
                clap::error::ErrorKind::ArgumentConflict,
                format!("the argument '{flag}' cannot be used with subcommand '{subcommand}'"),
            )
            .exit();
    }

    if cli.command.is_none()
        && !cli.with_ipc
        && (cli.ipc_bind.is_some() || cli.ipc_pid_file.is_some())
    {
        let flag = if cli.ipc_bind.is_some() {
            "--ipc-bind"
        } else {
            "--ipc-pid-file"
        };
        Cli::command()
            .error(
                clap::error::ErrorKind::MissingRequiredArgument,
                format!("the argument '{flag}' requires '--with-ipc'"),
            )
            .exit();
    }

    // Extract log_file from headless command (if applicable) and initialize
    // the logger once. Non-headless modes use the default stderr target.
    // In headless mode, also redirect stderr to the log file so that all
    // output (R device callbacks, eprintln!, etc.) is captured.
    let (log_file, is_headless) = match &cli.command {
        Some(Commands::Headless(args)) => (args.log_file.as_deref(), true),
        _ => (None, false),
    };
    init_logger(log_file, is_headless);

    // Install signal handlers for fatal signals (SIGSEGV, SIGILL, SIGBUS).
    // This prevents the process from hanging when R encounters a segmentation fault.
    // Must be called after init_logger so trap handlers can log.
    traps::register_trap_handlers();

    // Handle subcommands first
    match &cli.command {
        Some(Commands::Completions(args)) => {
            Cli::print_completions(args.shell);
            return Ok(());
        }
        Some(Commands::Config(args)) => {
            return handle_config_command(&args.action);
        }
        Some(Commands::History(args)) => {
            return handle_history_command(
                &args.action,
                cli.r_source.config.as_ref(),
                cli.history.history_dir.as_ref(),
            );
        }
        Some(Commands::Ipc(args)) => {
            handle_ipc_command(&args.action);
            return Ok(());
        }
        Some(Commands::Headless(args)) => {
            let r_args_builder = RArgsBuilder {
                vanilla: args.r_compat.vanilla,
                no_environ: args.r_compat.no_environ,
                no_site_file: args.r_compat.no_site_file,
                no_init_file: args.r_compat.no_init_file,
                save: false,
                restore: false,
                max_connections: args.r_compat.max_connections,
                max_ppsize: args.r_compat.max_ppsize,
                min_nsize: args.r_compat.min_nsize.as_deref(),
                min_vsize: args.r_compat.min_vsize.as_deref(),
            };
            return run_headless(
                args.r_source.config.as_ref(),
                args.r_source.r_home.as_deref(),
                args.r_source.r_version.as_deref(),
                r_args_builder,
                args.bind.as_deref(),
                args.pid_file.as_deref(),
                args.quiet,
                args.json,
                args.log_file.as_deref(),
                args.history.history_dir.as_deref(),
                args.history.no_history,
                args.r_source.no_r_source_overrides,
                &args.ipc_eval_allow_function,
                args.ipc_eval_unrestricted,
            );
        }
        Some(Commands::R(args)) => {
            let RCommand::Resolve(resolve_args) = &args.command;
            let origin = r_source_origin(&matches);
            return run_resolve(
                resolve_args.r_source.config.as_deref(),
                resolve_args.r_source.r_home.as_deref(),
                resolve_args.r_source.r_version.as_deref(),
                origin,
                resolve_args.r_source.no_r_source_overrides,
            );
        }
        None => {}
    }

    // Check if we're in script execution mode
    let script_mode = cli.eval.is_some() || cli.script_file().is_some();

    if script_mode {
        // Script execution mode - no REPL, just run code and exit
        return run_script(&cli);
    }

    log::info!("Starting arf");

    // Disable terminal input echo before startup work can receive extension
    // input, and restore the original mode on exit. R's quit() may bypass Rust
    // destructors, so the guard also registers an atexit fallback.
    #[cfg(unix)]
    let mut _console_mode_guard = console_mode::ConsoleModeGuard::install();
    #[cfg(not(unix))]
    let _console_mode_guard = console_mode::ConsoleModeGuard::install();

    // Ensure XDG directories exist
    ensure_directories()?;

    // Load configuration (from file or default)
    // Track the config path for :info command display
    let config_report = load_config_with_fallback(&cli);
    let mut config = config_report.config;
    let config_path = config_report.config_path;
    let config_status = config_report.status;
    let config_diagnostics = config_report.diagnostics;
    log::debug!("Loaded config: {:?}", config);

    // Apply CLI overrides
    if let Some(mode) = cli.reprex {
        let formatter = config.reprex.formatter;
        if mode == ReprexMode::Format && external::formatter::resolve_formatter(formatter).is_none()
        {
            let message = external::formatter::unavailable_message(
                formatter,
                external::formatter::FormatterUnavailableContext::ExplicitCli,
            );
            report_startup_diagnostics(config_diagnostics);
            anyhow::bail!("{message}");
        }
        config.startup.reprex = mode;
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

    let mut eval_allowlist = config.ipc.eval.allowed_functions.clone();
    eval_allowlist.extend(cli.ipc_eval_allow_function.iter().cloned());
    ipc::policy::set_policy(eval_allowlist, cli.ipc_eval_unrestricted);

    // History configuration: CLI flag overrides default XDG location
    config.history.mode = config::history_mode_with_overrides(
        &config.history.mode,
        cli.history.history_dir.as_deref(),
        cli.history.no_history,
    );

    // Configured format mode degrades to on when its formatter is unavailable.
    let formatter = config.reprex.formatter;
    let mut startup_diagnostics = config_diagnostics;
    if config.startup.reprex == ReprexMode::Format
        && cli.reprex.is_none()
        && external::formatter::resolve_formatter(formatter).is_none()
    {
        startup_diagnostics.push(StartupDiagnostic::user_warning(
            external::formatter::unavailable_message(
                formatter,
                external::formatter::FormatterUnavailableContext::ConfiguredMode,
            ),
        ));
        config.startup.reprex = ReprexMode::On;
    }

    // Set up R based on r_source config (with optional CLI override)
    let (resolution, startup_diagnostics) = report_diagnostics_on_setup_error(
        setup_r(
            &config.startup.r_source,
            &config.experimental.r_source_overrides,
            None,
            cli.r_source.r_home.as_deref(),
            cli.r_source.r_version.as_deref(),
            cli.r_source.no_r_source_overrides,
        ),
        startup_diagnostics,
        report_startup_diagnostics,
    )?;
    let r_source_status = resolution.status.clone();
    log::debug!("R source status: {:?}", r_source_status);

    // Ensure LD_LIBRARY_PATH includes R library directory.
    // This may re-exec the current process if the path needs updating.
    // On Unix, the pre-exec hook restores the terminal mode before exec so the
    // replacement process starts with the original mode. If exec fails (rare),
    // re-install the guard to re-disable echo for the rest of startup.
    //
    // A re-exec here inherits this process's environment, so the startup
    // snapshot carrier is set just before the call and removed right after it
    // returns. That way it only survives into the exec'd process when a
    // re-exec actually happens; if the path was already correct (no re-exec)
    // or the call failed, the carrier never lingers where R or a child
    // process could see it. Without this, a re-exec here would compute a
    // fresh snapshot instead of forwarding the one captured at the very start
    // of this process, silently overwriting it with whatever R changed the
    // variables to during the session.
    #[cfg(unix)]
    {
        // SAFETY: This runs during single-threaded startup, before any other
        // threads are spawned and before R is initialized, so mutating the
        // process environment here cannot race with a concurrent read.
        unsafe { std::env::set_var(STARTUP_ENV_CARRIER, startup_env_carrier()) };
        if let Some(fd) = pid_file::restart_fd_carrier() {
            // The descriptor itself is the only PID handoff capability. Keep
            // its number in the environment solely across this loader exec.
            unsafe { std::env::set_var(pid_file::RESTART_PID_FD_ENV, fd) };
        }
        if let Err(e) = arf_libr::ensure_ld_library_path_with_pre_exec_and_args(
            &normalized_args(),
            console_mode::restore_original_input_mode,
        ) {
            log::warn!("Could not set LD_LIBRARY_PATH: {}", e);
            // Drop old guard before calling install(): assignment evaluates the RHS
            // first (capturing and disabling echo), then drops the old guard, which
            // would call restore and re-enable echo. Explicit drop avoids that.
            drop(_console_mode_guard);
            _console_mode_guard = console_mode::ConsoleModeGuard::install();
        }
        // Reaching this line means no re-exec happened (or it failed), so the
        // carrier must not stay set for the rest of this process's lifetime.
        //
        // SAFETY: same single-threaded, pre-R-init context as above.
        unsafe { std::env::remove_var(STARTUP_ENV_CARRIER) };
        unsafe { std::env::remove_var(pid_file::RESTART_PID_FD_ENV) };
        pid_file::finish_loader_reexec();
    }
    #[cfg(not(unix))]
    if let Err(e) = arf_libr::ensure_ld_library_path() {
        log::warn!("Could not set LD_LIBRARY_PATH: {}", e);
    }

    // Report startup diagnostics only after the loader re-exec boundary. If
    // ensure_ld_library_path replaced this process, only the replacement
    // process reaches this point.
    report_startup_diagnostics(startup_diagnostics);
    resolution.emit_diagnostics();

    // Generate R initialization arguments from CLI flags
    let r_args = cli.r_args();
    let r_args_refs: Vec<&str> = r_args.iter().map(|s| s.as_str()).collect();
    log::debug!("R args: {:?}", r_args);

    // Install the Ctrl+C handler before R initialization: startup profiles
    // run inside setup_Rmainloop, and with R_SignalHandlers = 0 a SIGINT
    // during a slow .Rprofile would otherwise hit the default action and
    // kill the process. The handler is a no-op until the interrupt flag
    // pointer is resolved, which happens early in initialization, before
    // profiles are evaluated (see install_r_interrupt_handler).
    #[cfg(unix)]
    repl::install_r_interrupt_handler();

    // Initialize R with CLI-specified flags
    log::info!("Initializing R...");
    // Note the directory before initializing: on Unix, profiles run inside
    // initialization and may call setwd(), which would move the base a
    // relative R_HOME has to be resolved against.
    let pre_init_dir = std::env::current_dir().ok();
    let (r_initialized, r_home) = unsafe {
        match arf_libr::initialize_r_with_args(&r_args_refs) {
            Ok(()) => {
                log::info!("R initialized successfully");
                (true, capture_runtime_r_home(pre_init_dir.as_deref()))
            }
            Err(e) => {
                eprintln!("Warning: Failed to initialize R: {}", e);
                eprintln!("R evaluation will not be available.");
                eprintln!("Make sure R is installed and R_HOME is set correctly.\n");
                (false, None)
            }
        }
    };

    // If R initialization failed or the interrupt flag could not be
    // resolved, the handler installed above can never forward interrupts to
    // anything that consumes them and would swallow Ctrl+C forever; fall
    // back to the default disposition (terminate the process). The
    // r_initialized check matters even when the flag resolved: the flag
    // pointer is stored early in initialization, so a later failure would
    // otherwise leave the forwarding handler active with R disabled.
    #[cfg(unix)]
    if !r_initialized || !arf_libr::is_r_interrupt_flag_available() {
        log::warn!(
            "R initialization failed or interrupt flag not available; restoring \
             default Ctrl+C behavior (terminates the process)."
        );
        repl::restore_default_sigint_handler();
    }

    // Windows: install the Ctrl+C handler now, before profiles are sourced
    // below, so a SIGINT during a slow .Rprofile interrupts it instead of
    // killing the process (STATUS_CONTROL_C_EXIT).
    #[cfg(windows)]
    if arf_libr::is_r_interrupt_flag_available() {
        repl::install_r_interrupt_handler();
    } else {
        log::warn!(
            "R interrupt flag not available; skipping Ctrl+C handler installation. \
             Default console handler will terminate the process on Ctrl+C."
        );
    }

    // Source R profile files after R initialization (Windows only)
    // On Windows, R's built-in profile loading is disabled during initialization
    // (load_init_file = R_FALSE in arf-libr/src/sys.rs), so we must manually
    // source .Rprofile files here. On Unix, R handles this automatically.
    #[cfg(windows)]
    if r_initialized {
        source_r_profiles(&r_args);
    }

    // Static formals inspection relies on the library paths after R startup
    // profiles have run. Populate them only when runtime configuration opts in;
    // a failure is non-fatal because completion retains its R fallback.
    if r_initialized
        && matches!(
            config.experimental.r_completion.r#static.formals.mode,
            config::StaticFormalsMode::PreferStatic
        )
        && let Err(error) = arf_harp::lib_paths::populate_lib_paths()
    {
        log::warn!("Could not populate R library paths for static formals completion: {error}");
    }

    let session_id = create_session_id(&config);

    // Prepare both owned history runtimes before IPC advertises the session.
    // The server then exposes a store that is already the same owner used by
    // typed input and the interactive editor.
    let mut repl = Repl::new(
        config,
        config_path,
        config_status,
        r_source_status,
        r_home,
        session_id,
    )?;
    repl.prepare_history();

    // Start IPC server if requested.
    //
    // History runtimes and the R-owned IPC store were prepared above, so the
    // server advertises a session ID only after the R runtime is confirmed
    // available. The REPL later attaches those same owners to its editors.
    if cli.with_ipc {
        let ipc_bind = bind_path
            .as_deref()
            .map(|path| {
                path.to_str().ok_or_else(|| {
                    anyhow::anyhow!(
                        "IPC bind path is not valid UTF-8 and cannot be advertised by the IPC protocol"
                    )
                })
            })
            .transpose()?;
        match ipc::start_server(
            ipc_bind,
            repl.r_home_for_ipc(),
            None,
            repl.history_session_id_raw(),
            ipc::session::SessionType::Interactive,
        ) {
            Ok(session) => {
                log::info!("IPC server started on {}", session.socket_path);
                if let Some(pid_path) = &cli.ipc_pid_file {
                    let pid_path = absolute_pid_file_path(pid_path);
                    if let Err(e) = write_pid_file(&pid_path) {
                        ipc::stop_server();
                        return Err(e);
                    }
                    register_ipc_pid_file_atexit(&pid_path);
                }
            }
            Err(e) => {
                anyhow::bail!("Failed to start IPC server: {}", e);
            }
        }
    }

    // Run the REPL with runtimes prepared above.
    let repl_result = repl.run();

    // Cleanup IPC server on exit (idempotent — also covers :ipc start).
    // Called before propagating repl errors to ensure socket/session cleanup.
    ipc::stop_server();

    // Clean up PID file written by --ipc-pid-file.
    if let Some(pid_path) = &cli.ipc_pid_file {
        cleanup_ipc_pid_file(pid_path);
    }

    repl_result
}
