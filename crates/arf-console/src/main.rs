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
use app::commands::{handle_config_command, handle_history_command, handle_ipc_command};
use app::config_load::load_config_with_fallback;
use app::headless::run_headless;
#[cfg(windows)]
use app::r_profiles::source_r_profiles;
use app::resolve::{RSourceOrigin, ResolveCommandError, print_error, run_resolve};
use app::session_id::create_session_id;
use app::setup::{run_script, setup_r};
use clap::parser::ValueSource;
use clap::{ArgMatches, Command, CommandFactory, FromArgMatches};
use cli::{Cli, Commands, RArgsBuilder, RCommand};
use config::ensure_directories;
use logging::init_logger;
use pid_file::{
    absolute_pid_file_path, cleanup_ipc_pid_file, register_ipc_pid_file_atexit, write_pid_file,
};
use repl::Repl;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::OnceLock;

const STARTUP_ENV_VARS: &[&str] = &[
    "R_HOME",
    "LD_LIBRARY_PATH",
    "R_LIBS_USER",
    "R_LIBS_SITE",
    "R_LIBS",
    "R_DOC_DIR",
    "R_SHARE_DIR",
    "R_INCLUDE_DIR",
    "R_SYSTEM_ABI",
];

static STARTUP_ENV: OnceLock<HashMap<String, OsString>> = OnceLock::new();

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
    let (mut config, config_path, config_status) = load_config_with_fallback(&cli);
    log::debug!("Loaded config: {:?}", config);

    // Apply CLI overrides
    if cli.reprex {
        config.startup.mode.reprex = true;
    }
    if cli.auto_format {
        if !external::formatter::is_formatter_available() {
            anyhow::bail!(
                "Cannot enable auto-format: Air CLI ('air' command) not found in PATH.\n\
                 Install Air CLI from https://github.com/posit-dev/air"
            );
        }
        config.startup.mode.autoformat = true;
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
    if cli.history.no_history {
        config.history.disabled = true;
    } else if let Some(history_dir) = &cli.history.history_dir {
        config.history.dir = Some(history_dir.clone());
    }

    // Warn if auto-format is enabled (via config) but Air CLI is not available
    if config.startup.mode.autoformat
        && !cli.auto_format
        && !external::formatter::is_formatter_available()
    {
        eprintln!(
            "Warning: Auto-format is enabled in config but Air CLI ('air' command) not found in PATH."
        );
        eprintln!(
            "         Auto-format has been disabled. Install Air CLI from https://github.com/posit-dev/air"
        );
        config.startup.mode.autoformat = false;
    }

    // Set up R based on r_source config (with optional CLI override)
    let resolution = setup_r(
        &config.startup.r_source,
        &config.experimental.r_source_overrides,
        None,
        cli.r_source.r_home.as_deref(),
        cli.r_source.r_version.as_deref(),
        cli.r_source.no_r_source_overrides,
    )?;
    resolution.emit_diagnostics();
    let r_source_status = resolution.status;
    log::debug!("R source status: {:?}", r_source_status);

    // Ensure LD_LIBRARY_PATH includes R library directory.
    // This may re-exec the current process if the path needs updating.
    // On Unix, the pre-exec hook restores the terminal mode before exec so the
    // replacement process starts with the original mode. If exec fails (rare),
    // re-install the guard to re-disable echo for the rest of startup.
    #[cfg(unix)]
    if let Err(e) =
        arf_libr::ensure_ld_library_path_with_pre_exec(console_mode::restore_original_input_mode)
    {
        log::warn!("Could not set LD_LIBRARY_PATH: {}", e);
        // Drop old guard before calling install(): assignment evaluates the RHS
        // first (capturing and disabling echo), then drops the old guard, which
        // would call restore and re-enable echo. Explicit drop avoids that.
        drop(_console_mode_guard);
        _console_mode_guard = console_mode::ConsoleModeGuard::install();
    }
    #[cfg(not(unix))]
    if let Err(e) = arf_libr::ensure_ld_library_path() {
        log::warn!("Could not set LD_LIBRARY_PATH: {}", e);
    }

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

    let session_id = create_session_id(&config);
    let session_id_raw = session_id.map(i64::from);

    // Register history DB path for IPC history queries.
    // Note: the DB file may not exist yet at this point (first run); that's OK
    // because SqliteBackedHistory::with_file creates it on open. In the REPL
    // path, reedline opens the DB later in Repl::run_*, which also creates it.
    if !config.history.disabled {
        let history_dir = config.history.dir.clone().or_else(config::history_dir);
        if let Some(dir) = history_dir {
            ipc::set_history_db_info(dir.join("r.db"), session_id);
        }
    }

    // Start IPC server if requested.
    //
    // NOTE: The IPC server is started before history databases are opened (which
    // happens inside `Repl::run_*`).  This means there is a brief window where
    // the on-disk session file advertises a non-null `history_session_id` even
    // though history has not been confirmed yet.  If history initialization later
    // fails, `clear_history_session_id()` is called to set it back to `null`.
    // In practice the window is negligibly short (milliseconds).
    if cli.with_ipc {
        match ipc::start_server(
            cli.ipc_bind.as_deref(),
            r_home.as_ref().map(|path| path.display().to_string()),
            None,
            session_id_raw,
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

    // Create and run the REPL
    let mut repl = Repl::new(
        config,
        config_path,
        config_status,
        r_source_status,
        r_home,
        session_id,
    )?;
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

/// Capture the environment inherited by arf before R initialization can add
/// any R-specific variables.
fn capture_startup_env() {
    let snapshot = STARTUP_ENV_VARS
        .iter()
        .filter_map(|name| std::env::var_os(name).map(|value| ((*name).to_string(), value)))
        .collect();
    let _ = STARTUP_ENV.set(snapshot);
}

/// Return the value of a variable from the environment inherited at startup.
pub(crate) fn startup_env_value(name: &str) -> Option<OsString> {
    STARTUP_ENV.get()?.get(name).cloned()
}

/// Capture the R_HOME reported by the initialized R runtime.
///
/// This is intentionally called only after successful R initialization. The
/// startup resolution report is a prediction, while `R.home()` is a fact from
/// the runtime that is actually in use.
///
/// `base_dir` is the working directory from before initialization, used to
/// resolve a relative R_HOME. See [`absolutize_runtime_r_home`].
fn capture_runtime_r_home(base_dir: Option<&std::path::Path>) -> Option<PathBuf> {
    let r_home = match unsafe { arf_harp::eval_r_to_string(r#"base::R.home()"#) } {
        Ok(Some(r_home)) => r_home,
        Ok(None) => {
            log::warn!("R.home() returned NULL after R initialization");
            return None;
        }
        Err(error) => {
            log::warn!("Could not evaluate R.home() after R initialization: {error}");
            return None;
        }
    };

    absolutize_runtime_r_home(PathBuf::from(r_home), base_dir)
}

/// Resolve a relative R_HOME against the directory R started in.
///
/// R stores whatever R_HOME string it was given, so `R.home()` can be
/// relative. The base has to be the working directory from before
/// initialization: a `.Rprofile` runs during initialization on Unix and may
/// call `setwd()`, and resolving against the directory afterwards would name a
/// different location than the one R loaded from. Without a base, a relative
/// path is dropped rather than guessed at.
///
/// Paths are joined, never canonicalized: resolving symlinks would report an
/// R_HOME other than the one in use. `arf r resolve` joins for the same reason.
///
/// In practice R fails to initialize with a relative R_HOME, so this is
/// defensive. It is kept because arf does not reject relative input either: an
/// explicit `--r-home` is returned unchanged when the path has no `bin/R`.
fn absolutize_runtime_r_home(
    r_home: PathBuf,
    base_dir: Option<&std::path::Path>,
) -> Option<PathBuf> {
    if r_home.is_absolute() {
        return Some(r_home);
    }
    match base_dir {
        Some(base) => Some(base.join(r_home)),
        None => {
            log::warn!(
                "Could not make R.home() absolute: the directory R started in is unknown ({})",
                r_home.display()
            );
            None
        }
    }
}

fn r_source_origin(matches: &ArgMatches) -> Option<RSourceOrigin> {
    let resolve_matches = matches
        .subcommand_matches("r")
        .and_then(|matches| matches.subcommand_matches("resolve"))?;

    if resolve_matches.value_source("r_home") == Some(ValueSource::CommandLine)
        || resolve_matches.value_source("r_version") == Some(ValueSource::CommandLine)
    {
        Some(RSourceOrigin::Cli)
    } else if resolve_matches.value_source("r_home") == Some(ValueSource::EnvVariable)
        || resolve_matches.value_source("r_version") == Some(ValueSource::EnvVariable)
    {
        Some(RSourceOrigin::Environment)
    } else {
        None
    }
}

fn validate_top_level_scope(command: &Command, matches: &ArgMatches) {
    if matches.subcommand_name().is_none() {
        return;
    }

    let mut path = Vec::new();
    let mut path_commands = Vec::new();
    let mut current_command = command;
    let mut current_matches = matches;

    while let Some((subcommand_name, nested_matches)) = current_matches.subcommand() {
        let subcommand = current_command
            .find_subcommand(subcommand_name)
            .expect("parsed subcommand must exist");
        path.push(subcommand_name.to_owned());
        path_commands.push(subcommand);
        current_command = subcommand;
        current_matches = nested_matches;
    }

    let subcommand_path = path.join(" ");
    let final_subcommand = path_commands
        .last()
        .map(|subcommand| (*subcommand).clone())
        .expect("parsed subcommand path must not be empty");

    for arg in command.get_arguments() {
        let Some(long) = arg.get_long() else {
            continue;
        };

        // These checks have deliberately custom errors below and must retain
        // their existing wording and ordering.
        if matches!(arg.get_id().as_str(), "eval" | "file")
            || is_history_option_allowed(&path, long)
        {
            continue;
        }

        if matches.value_source(arg.get_id().as_str()) != Some(ValueSource::CommandLine) {
            continue;
        }

        let mut subcommand_command = final_subcommand.clone();
        subcommand_command.set_bin_name(format!("arf {subcommand_path}"));

        if let Some(subcommand_arg) = path_commands.iter().rev().find_map(|subcommand| {
            subcommand
                .get_arguments()
                .find(|subcommand_arg| subcommand_arg.get_long() == Some(long))
        }) {
            let value_names = if matches!(
                subcommand_arg.get_action(),
                clap::ArgAction::SetTrue | clap::ArgAction::SetFalse
            ) {
                String::new()
            } else {
                subcommand_arg
                    .get_value_names()
                    .map(|names| {
                        names
                            .iter()
                            .map(|name| format!("<{}>", name.as_str()))
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default()
            };
            let corrected_form = if value_names.is_empty() {
                format!("arf {subcommand_path} --{long}")
            } else {
                format!("arf {subcommand_path} --{long} {value_names}")
            };

            subcommand_command
                .error(
                    clap::error::ErrorKind::ArgumentConflict,
                    format!(
                        "'--{long}' was given before the '{subcommand_path}' subcommand, where it has no effect\n\n  tip: place it after the subcommand instead:\n       {corrected_form}"
                    ),
                )
                .exit();
        }

        let console_form = format!("arf --{long}");
        subcommand_command
            .error(
                clap::error::ErrorKind::ArgumentConflict,
                format!(
                    "'--{long}' is not used by the '{subcommand_path}' subcommand\n\n  tip: it applies to the interactive console, which takes no subcommand:\n       {console_form}"
                ),
            )
            .exit();
    }
}

fn is_history_option_allowed(path: &[String], long: &str) -> bool {
    matches!(long, "config" | "history-dir")
        && path.len() == 2
        && path[0] == "history"
        && matches!(path[1].as_str(), "import" | "export")
}

#[cfg(test)]
mod tests {
    use super::absolutize_runtime_r_home;
    use std::path::{Path, PathBuf};

    #[test]
    fn absolute_runtime_r_home_is_left_alone() {
        let r_home = PathBuf::from(if cfg!(windows) {
            r"C:\opt\R\lib\R"
        } else {
            r"/opt/R/lib/R"
        });
        assert_eq!(
            absolutize_runtime_r_home(r_home.clone(), Some(Path::new("/elsewhere"))),
            Some(r_home)
        );
    }

    #[test]
    fn relative_runtime_r_home_resolves_against_the_startup_directory() {
        // The point of taking the base as an argument: a setwd() during
        // initialization cannot influence the result.
        assert_eq!(
            absolutize_runtime_r_home(PathBuf::from("lib/R"), Some(Path::new("/start"))),
            Some(PathBuf::from("/start/lib/R"))
        );
    }

    #[test]
    fn relative_runtime_r_home_without_a_base_is_dropped() {
        assert_eq!(
            absolutize_runtime_r_home(PathBuf::from("lib/R"), None),
            None
        );
    }
}
