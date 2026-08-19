//! Command-line interface definition using clap.

mod completions;
mod config;
mod headless;
mod history;
mod ipc;
mod r_args;
mod resolve;
mod shared;

pub(crate) use config::ConfigAction;
pub(crate) use history::{HistoryAction, ImportSource};
pub(crate) use ipc::IpcAction;
pub(crate) use r_args::RArgsBuilder;
pub(crate) use resolve::RCommand;

use clap::{ArgAction, Parser, Subcommand, ValueHint};
#[cfg(test)]
use clap_complete::Shell;
use std::path::PathBuf;

/// A cross-platform R console written in Rust.
#[derive(Parser, Debug)]
#[command(name = "arf")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// Evaluate R expression and exit
    #[arg(short = 'e', long = "eval")]
    pub eval: Option<String>,

    /// [R] Take input from FILE
    #[arg(short = 'f', long = "file", value_hint = ValueHint::FilePath, conflicts_with = "eval", hide_short_help = true)]
    pub file: Option<PathBuf>,

    /// Set reprex mode (off, on, or format).
    ///
    /// Config: startup.reprex
    #[arg(long, value_name = "MODE")]
    pub reprex: Option<crate::config::ReprexMode>,

    /// Shared R source options
    #[command(flatten)]
    pub r_source: shared::RSourceArgs,

    /// Suppress the startup banner
    ///
    /// Config: show_banner
    #[arg(long)]
    pub no_banner: bool,

    // R-compatible flags; see shared::RCompatArgs.
    #[command(flatten)]
    pub r_compat: shared::RCompatArgs,

    /// [R] Don't print R startup message
    #[arg(short = 'q', long = "quiet", alias = "silent", hide_short_help = true)]
    pub quiet: bool,

    /// [R] Don't save workspace at end of session (default)
    #[arg(long = "no-save", hide_short_help = true)]
    pub no_save: bool,

    /// [R] Save workspace at end of session
    #[arg(long = "save", conflicts_with = "no_save", hide_short_help = true)]
    pub save: bool,

    /// [R] Don't restore previously saved objects (default)
    #[arg(long = "no-restore", hide_short_help = true)]
    pub no_restore: bool,

    /// [R] Don't restore previously saved objects
    #[arg(long = "no-restore-data", hide_short_help = true)]
    pub no_restore_data: bool,

    /// [R] Restore previously saved objects
    #[arg(long = "restore-data", conflicts_with_all = ["no_restore", "no_restore_data"], hide_short_help = true)]
    pub restore_data: bool,

    /// [R] Force R to run interactively (no-op, always interactive)
    #[arg(long = "interactive", hide = true)]
    pub interactive: bool,

    /// [R] Don't echo input (no-op, arf controls its own echo)
    #[arg(long = "no-echo", hide_short_help = true)]
    pub no_echo: bool,

    /// [R] Combine --quiet --no-save --no-restore (deprecated in R 4.0, use --no-echo)
    #[arg(long = "slave", hide = true)]
    pub slave: bool,

    /// [R] Restore previously saved objects (opposite of --no-restore)
    #[arg(long = "restore", conflicts_with_all = ["no_restore", "no_restore_data"], hide = true)]
    pub restore: bool,

    /// [R] Print more information about progress (no-op)
    #[arg(long = "verbose", hide = true)]
    pub verbose: bool,

    /// [R] Specify encoding to be used for stdin (no-op)
    #[arg(long = "encoding", hide = true)]
    pub encoding: Option<String>,

    /// [R] Run R through debugger NAME (no-op)
    #[arg(short = 'd', long = "debugger", hide = true)]
    pub debugger: Option<String>,

    /// [R] Pass ARGS as arguments to the debugger (no-op)
    #[arg(long = "debugger-args", hide = true)]
    pub debugger_args: Option<String>,

    /// [R] Use TYPE as GUI (no-op)
    #[arg(short = 'g', long = "gui", hide = true)]
    pub gui: Option<String>,

    /// [R] Specify a sub-architecture (no-op)
    #[arg(long = "arch", hide = true)]
    pub arch: Option<String>,

    /// [R] In R, skip the rest of the command line.
    /// arf accepts this flag for compatibility but does NOT consume trailing arguments;
    /// unknown flags after --args will still cause a parse error.
    #[arg(long = "args", hide = true, num_args = 0)]
    pub r_args_marker: bool,

    /// [R] Don't use readline (no-op)
    #[arg(long = "no-readline", hide = true)]
    pub no_readline: bool,

    /// [R] Don't restore history (no-op)
    #[arg(long = "no-restore-history", hide = true)]
    pub no_restore_history: bool,

    /// Enable IPC server for external tool access (AI agents, vscode-R, etc.)
    ///
    /// Starts the IPC server alongside the interactive REPL, allowing
    /// external tools to call `arf ipc eval`, `arf ipc send`, etc.
    /// For headless (no REPL) usage, see `arf headless` instead.
    #[arg(long = "with-ipc")]
    pub with_ipc: bool,

    /// Bind IPC socket to a specific path instead of the default (requires --with-ipc)
    ///
    /// Unix: filesystem path (e.g. /tmp/my-arf.sock)
    /// Windows: named pipe path (e.g. \\.\pipe\my-arf)
    #[arg(long = "ipc-bind", value_hint = ValueHint::FilePath)]
    pub ipc_bind: Option<String>,

    /// Write server PID to a file on startup (requires --with-ipc)
    ///
    /// The file is removed when the REPL exits. Startup fails if the file
    /// cannot be written, so the editor is guaranteed to own the session or
    /// get an error — there is no silent fallback.
    #[arg(long = "ipc-pid-file", value_hint = ValueHint::FilePath)]
    pub ipc_pid_file: Option<PathBuf>,

    /// Add an exact function target to the IPC evaluate allowlist. May be
    /// repeated; package-qualified targets use `package::function`.
    #[arg(
        long = "ipc-eval-allow-function",
        action = ArgAction::Append
    )]
    pub ipc_eval_allow_function: Vec<String>,

    /// Disable the IPC evaluate allowlist for this server startup only.
    #[arg(long = "ipc-eval-unrestricted")]
    pub ipc_eval_unrestricted: bool,

    /// Disable auto-matching of brackets and quotes (for testing)
    #[arg(long = "no-auto-match", hide = true)]
    pub no_auto_match: bool,

    /// Disable completion menu (for testing)
    #[arg(long = "no-completion", hide = true)]
    pub no_completion: bool,

    #[command(flatten)]
    pub history: shared::HistoryOptions,

    /// Subcommands
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Generate shell completion scripts
    Completions(completions::CompletionsArgs),
    /// Configuration management
    Config(config::ConfigArgs),
    /// History management
    History(history::HistoryArgs),
    /// Interact with a running arf session via IPC
    ///
    /// Evaluate R code, send user input, or query session info in a running
    /// arf instance. The target session must have IPC enabled — via
    /// `arf headless`, `arf --with-ipc`, or the `:ipc start` meta command.
    #[command(after_long_help = "\
Quick start:
  # 1. Start a headless session (or use --with-ipc with the REPL)
  $ arf headless --ipc-eval-allow-function '+' &

  # 2. Check the session is running
  $ arf ipc list

  # 3. Evaluate R code and get structured JSON output
  $ arf ipc eval '1 + 1'

  # 4. Check session status (R version, loaded packages, etc.)
  $ arf ipc session

  # 5. Shut down when done
  $ arf ipc shutdown

All commands output JSON to stdout (pretty-printed on terminal, compact \
when piped). Errors are written to stderr as JSON. Exit codes: \
0 = success, 2 = client-side failure (transport error or missing/unreadable \
code input), 3 = session error, 4 = protocol error.

Session discovery:
  By default, sessions are discovered from the platform cache directory.
  Set ARF_IPC_SESSIONS_DIR to explicitly override the session metadata \
directory for all IPC writers (`arf headless`, `--with-ipc`, `:ipc start`) \
and `arf ipc` readers.")]
    Ipc(ipc::IpcArgs),
    /// Run R with IPC server only (no interactive REPL)
    ///
    /// Starts R and an IPC server without the interactive console.
    /// Useful for AI agents that only need IPC access, or for
    /// CI environments where a terminal is not available.
    /// Exit with Ctrl+C or `arf ipc shutdown`.
    #[command(after_long_help = "\
Examples:
  Start headless and evaluate R code:
    $ arf headless --ipc-eval-allow-function '+' --ipc-eval-allow-function 'Sys.time' &
    # (wait for the server to be ready)
    $ arf ipc eval '1 + 1'

  CI usage with JSON output:
    $ arf headless --json | jq -r .socket_path

  Run with logging to a file:
    $ arf headless --log-file arf.log --ipc-pid-file arf.pid

  Use a custom socket path:
    $ arf headless --ipc-bind /tmp/my-arf.sock --ipc-pid-file arf.pid \\
        --ipc-eval-allow-function 'Sys.time' &
    $ arf ipc eval --pid $(cat arf.pid) 'Sys.time()'

  Shut down a headless session:
    $ arf ipc shutdown")]
    // Boxed to keep this variant from dominating the size of the whole enum.
    Headless(Box<headless::HeadlessArgs>),
    /// R source resolution commands
    R(resolve::RArgs),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completions_bash_snapshot() {
        let completions = Cli::generate_completions_string(Shell::Bash);
        insta::with_settings!({snapshot_path => "../snapshots"}, {
            insta::assert_snapshot!("completions_bash", completions);
        });
    }

    #[test]
    fn test_completions_zsh_snapshot() {
        let completions = Cli::generate_completions_string(Shell::Zsh);
        insta::with_settings!({snapshot_path => "../snapshots"}, {
            insta::assert_snapshot!("completions_zsh", completions);
        });
    }

    #[test]
    fn test_completions_fish_snapshot() {
        let completions = Cli::generate_completions_string(Shell::Fish);
        insta::with_settings!({snapshot_path => "../snapshots"}, {
            insta::assert_snapshot!("completions_fish", completions);
        });
    }

    #[test]
    fn test_completions_powershell_snapshot() {
        let completions = Cli::generate_completions_string(Shell::PowerShell);
        insta::with_settings!({snapshot_path => "../snapshots"}, {
            insta::assert_snapshot!("completions_powershell", completions);
        });
    }

    #[test]
    fn test_help_history_import_snapshot() {
        let help = Cli::generate_help_string(&["history", "import"]);
        insta::with_settings!({snapshot_path => "../snapshots"}, {
            insta::assert_snapshot!("help_history_import", help);
        });
    }

    #[test]
    fn test_help_headless_snapshot() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        guard.unset("ARF_HISTORY_DIR");
        let help = Cli::generate_help_string(&["headless"]);
        insta::with_settings!({snapshot_path => "../snapshots"}, {
            insta::assert_snapshot!("help_headless", help);
        });
    }

    #[test]
    fn test_help_history_export_snapshot() {
        let help = Cli::generate_help_string(&["history", "export"]);
        insta::with_settings!({snapshot_path => "../snapshots"}, {
            insta::assert_snapshot!("help_history_export", help);
        });
    }

    // clap renders the current value of an option's environment variable into
    // the long help, so this snapshot only holds if those variables are unset.
    // Clearing them keeps the test independent of the surrounding environment;
    // the environment guard keeps the tests below from setting them concurrently.
    #[test]
    fn test_help_long_snapshot() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        guard.unset("ARF_HISTORY_DIR");

        let help = Cli::generate_help_string(&[]);
        insta::with_settings!({snapshot_path => "../snapshots"}, {
            insta::assert_snapshot!("help_long", help);
        });
    }

    #[test]
    fn test_history_dir_rejects_empty_string() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        let result = Cli::try_parse_from(["arf", "--history-dir", ""]);
        assert!(result.is_err(), "empty --history-dir should be rejected");
    }

    #[test]
    fn test_no_r_source_overrides_flag_is_available_on_normal_cli() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        let cli = Cli::try_parse_from(["arf", "--no-r-source-overrides"]).unwrap();
        assert!(cli.r_source.no_r_source_overrides);
    }

    #[test]
    fn test_no_r_source_overrides_flag_is_available_on_headless_cli() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        let cli = Cli::try_parse_from(["arf", "headless", "--no-r-source-overrides"]).unwrap();
        let Some(Commands::Headless(args)) = cli.command else {
            panic!("expected headless command");
        };
        assert!(args.r_source.no_r_source_overrides);
    }

    #[test]
    fn test_r_resolve_subcommand_has_its_own_resolution_flags() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        let cli = Cli::try_parse_from([
            "arf",
            "r",
            "resolve",
            "--r-home",
            "/tmp/r-home",
            "--no-r-source-overrides",
            "--config",
            "/tmp/arf.toml",
        ])
        .unwrap();

        let Some(Commands::R(args)) = cli.command else {
            panic!("expected r command");
        };
        let RCommand::Resolve(args) = args.command;

        assert_eq!(args.r_source.config, Some(PathBuf::from("/tmp/arf.toml")));
        assert_eq!(args.r_source.r_home, Some(PathBuf::from("/tmp/r-home")));
        assert!(args.r_source.no_r_source_overrides);
        assert!(args.r_source.r_version.is_none());
    }

    #[test]
    fn test_no_r_source_overrides_does_not_conflict_with_r_home() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        let cli =
            Cli::try_parse_from(["arf", "--no-r-source-overrides", "--r-home", "/tmp/r-home"]);
        assert!(cli.is_ok());
    }

    #[test]
    fn test_arf_r_home_has_same_precedence_as_r_home() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        guard.set("ARF_R_HOME", "/env/r-home");
        let cli = Cli::try_parse_from(["arf"]).unwrap();

        assert_eq!(
            cli.r_source.r_home.as_deref(),
            Some(std::path::Path::new("/env/r-home"))
        );
    }

    #[test]
    fn test_arf_r_version_has_same_precedence_as_with_r_version() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        guard.set("ARF_R_VERSION", "4.5");
        let cli = Cli::try_parse_from(["arf"]).unwrap();

        assert_eq!(cli.r_source.r_version.as_deref(), Some("4.5"));
    }

    #[test]
    fn test_cli_value_wins_over_r_source_environment_value() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        guard.set("ARF_R_HOME", "/env/r-home");
        let home_cli = Cli::try_parse_from(["arf", "--r-home", "/cli/r-home"]).unwrap();
        assert_eq!(
            home_cli.r_source.r_home.as_deref(),
            Some(std::path::Path::new("/cli/r-home"))
        );

        guard.unset("ARF_R_HOME");
        guard.set("ARF_R_VERSION", "4.4");
        let version_cli = Cli::try_parse_from(["arf", "--with-r-version", "4.5"]).unwrap();
        assert_eq!(version_cli.r_source.r_version.as_deref(), Some("4.5"));
    }

    #[test]
    fn test_arf_r_home_conflicts_with_with_r_version() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        guard.set("ARF_R_HOME", "/env/r-home");
        let result = Cli::try_parse_from(["arf", "--with-r-version", "4.5"]);

        assert!(result.is_err());
    }

    #[test]
    fn test_headless_r_source_flags_read_environment_values() {
        let mut guard = crate::test_utils::lock_env();
        guard.set("ARF_R_HOME", "/env/r-home");
        guard.unset("ARF_R_VERSION");
        let cli = Cli::try_parse_from(["arf", "headless"]).unwrap();
        let Some(Commands::Headless(args)) = cli.command else {
            panic!("expected headless command");
        };
        assert_eq!(args.r_source.r_home, Some(PathBuf::from("/env/r-home")));

        guard.unset("ARF_R_HOME");
        guard.set("ARF_R_VERSION", "4.5");
        let cli = Cli::try_parse_from(["arf", "r", "resolve"]).unwrap();
        let Some(Commands::R(args)) = cli.command else {
            panic!("expected r command");
        };
        let RCommand::Resolve(args) = args.command;
        assert_eq!(args.r_source.r_version.as_deref(), Some("4.5"));
    }

    #[test]
    fn test_headless_r_source_flags_conflict_with_environment_values() {
        let mut guard = crate::test_utils::lock_env();
        guard.set("ARF_R_HOME", "/env/r-home");
        guard.unset("ARF_R_VERSION");
        assert!(Cli::try_parse_from(["arf", "headless", "--with-r-version", "4.5"]).is_err());

        guard.unset("ARF_R_HOME");
        guard.set("ARF_R_VERSION", "4.5");
        assert!(Cli::try_parse_from(["arf", "r", "resolve", "--r-home", "/cli"]).is_err());
    }

    #[test]
    fn test_headless_history_dir_reads_environment_value() {
        let mut guard = crate::test_utils::lock_env();
        guard.unset("ARF_R_HOME");
        guard.unset("ARF_R_VERSION");
        guard.set("ARF_HISTORY_DIR", "/env/history");
        let cli = Cli::try_parse_from(["arf", "headless"]).unwrap();
        let Some(Commands::Headless(args)) = cli.command else {
            panic!("expected headless command");
        };
        assert_eq!(
            args.history.history_dir,
            Some(PathBuf::from("/env/history"))
        );
    }
}
