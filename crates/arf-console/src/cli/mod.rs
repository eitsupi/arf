//! Command-line interface definition using clap.

mod completions;
mod config;
mod headless;
mod history;
mod ipc;
mod r_args;
mod r_home;

pub(crate) use config::ConfigAction;
pub(crate) use history::{HistoryAction, ImportSource};
pub(crate) use ipc::IpcAction;
pub(crate) use r_args::RArgsBuilder;

use clap::builder::TypedValueParser;
use clap::{Parser, Subcommand, ValueHint};
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

    /// Enable reprex mode (no prompt, output prefixed with #>)
    ///
    /// Config: startup.mode.reprex
    #[arg(long)]
    pub reprex: bool,

    /// Enable auto-formatting of R code in reprex mode (requires Air CLI)
    ///
    /// Config: startup.mode.autoformat
    #[arg(long)]
    pub auto_format: bool,

    /// Path to configuration file
    #[arg(short, long, value_hint = ValueHint::FilePath)]
    pub config: Option<PathBuf>,

    /// Suppress the startup banner
    ///
    /// Config: show_banner
    #[arg(long)]
    pub no_banner: bool,

    /// Highest-priority R source: use this R version via rig
    ///
    /// Accepts a rig alias (e.g. "release"), "default", a rig-assigned name,
    /// a full version ("4.4.1"), a partial version ("4.4" or "4", matching
    /// the latest release in that series), or a version range in the style
    /// Cargo and npm use ("^4.4", ">=4.3, <5.0").
    ///
    /// Requires rig. Candidates are limited to R versions rig has installed
    /// (from `rig list --json`); the version string is never passed to rig.
    ///
    /// Takes precedence over ARF_R_VERSION, r_source_overrides and
    /// startup.r_source, which are not consulted at all when this is set.
    ///
    /// Config: startup.r_source
    #[arg(
        long = "with-r-version",
        env = "ARF_R_VERSION",
        conflicts_with = "r_home"
    )]
    pub r_version: Option<String>,

    /// Highest-priority R source: use this explicit R_HOME path
    ///
    /// Mutually exclusive with --with-r-version and ARF_R_VERSION.
    ///
    /// Takes precedence over ARF_R_HOME, r_source_overrides and
    /// startup.r_source, which are not consulted at all when this is set.
    ///
    /// Config: startup.r_source
    #[arg(long = "r-home", value_hint = ValueHint::DirPath, env = "ARF_R_HOME", conflicts_with = "r_version")]
    pub r_home: Option<PathBuf>,

    /// Disable experimental directory-level R source overrides
    ///
    /// This only disables r_source_overrides. An R source given by
    /// --r-home, --with-r-version, ARF_R_HOME or ARF_R_VERSION still applies.
    ///
    /// Config: [experimental].r_source_overrides
    #[arg(long = "no-r-source-overrides")]
    pub no_r_source_overrides: bool,

    // R-compatible flags (passed to R, for vscode-R and radian compatibility)
    // Hidden from short help (-h) but shown in long help (--help).
    /// Start R in vanilla mode (no init files, no save/restore)
    #[arg(long = "vanilla")]
    pub vanilla: bool,

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

    /// [R] Don't read the site and user environment files
    #[arg(long = "no-environ", hide_short_help = true)]
    pub no_environ: bool,

    /// [R] Don't read the site-wide Rprofile
    #[arg(long = "no-site-file", hide_short_help = true)]
    pub no_site_file: bool,

    /// [R] Don't read the user's .Rprofile
    #[arg(long = "no-init-file", hide_short_help = true)]
    pub no_init_file: bool,

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

    /// [R] Set max number of connections to N
    #[arg(long = "max-connections", hide = true)]
    pub max_connections: Option<u32>,

    /// [R] Set max size of protect stack to N
    #[arg(long = "max-ppsize", hide = true)]
    pub max_ppsize: Option<u32>,

    /// [R] Set min number of fixed size obj's ("cons cells") to N
    #[arg(long = "min-nsize", hide = true)]
    pub min_nsize: Option<String>,

    /// [R] Set vector heap minimum to N bytes; '4M' = 4 MegaB
    #[arg(long = "min-vsize", hide = true)]
    pub min_vsize: Option<String>,

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

    /// Disable auto-matching of brackets and quotes (for testing)
    #[arg(long = "no-auto-match", hide = true)]
    pub no_auto_match: bool,

    /// Disable completion menu (for testing)
    #[arg(long = "no-completion", hide = true)]
    pub no_completion: bool,

    /// Custom history directory (overrides default XDG location)
    ///
    /// R history will be stored at `{dir}/r.db`, Shell at `{dir}/shell.db`.
    ///
    /// Config: history.dir
    #[arg(
        long = "history-dir",
        value_hint = ValueHint::DirPath,
        env = "ARF_HISTORY_DIR",
        hide_short_help = true,
        value_parser = clap::builder::NonEmptyStringValueParser::new().map(PathBuf::from),
    )]
    pub history_dir: Option<PathBuf>,

    /// Disable history (no history saved or loaded)
    ///
    /// Config: history.disabled
    #[arg(long = "no-history", hide_short_help = true)]
    pub no_history: bool,

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
  $ arf headless &

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
    $ arf headless &
    # (wait for the server to be ready)
    $ arf ipc eval '1 + 1'

  CI usage with JSON output:
    $ arf headless --json | jq -r .socket_path

  Run with logging to a file:
    $ arf headless --log-file arf.log --ipc-pid-file arf.pid

  Use a custom socket path:
    $ arf headless --ipc-bind /tmp/my-arf.sock --ipc-pid-file arf.pid
    $ arf ipc eval --pid $(cat arf.pid) 'Sys.time()'

  Shut down a headless session:
    $ arf ipc shutdown")]
    Headless(headless::HeadlessArgs),
    /// Print the R_HOME path arf would use without starting R
    RHome(r_home::RHomeArgs),
}

#[cfg(test)]
mod tests {
    use super::*;

    struct EnvVarGuard {
        name: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(name: &'static str, value: &str) -> Self {
            let original = std::env::var_os(name);
            // SAFETY: Tests serialize access to these process-global variables.
            unsafe { std::env::set_var(name, value) };
            Self { name, original }
        }

        fn unset(name: &'static str) -> Self {
            let original = std::env::var_os(name);
            // SAFETY: Tests serialize access to these process-global variables.
            unsafe { std::env::remove_var(name) };
            Self { name, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            // SAFETY: Tests serialize access to these process-global variables.
            unsafe {
                if let Some(value) = &self.original {
                    std::env::set_var(self.name, value);
                } else {
                    std::env::remove_var(self.name);
                }
            }
        }
    }

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
    #[serial_test::serial]
    fn test_help_headless_snapshot() {
        let _r_home = EnvVarGuard::unset("ARF_R_HOME");
        let _r_version = EnvVarGuard::unset("ARF_R_VERSION");
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
    // Clearing them keeps the test independent of the surrounding environment,
    // and serializing keeps the tests below from setting them concurrently.
    #[test]
    #[serial_test::serial]
    fn test_help_long_snapshot() {
        let _r_home = EnvVarGuard::unset("ARF_R_HOME");
        let _r_version = EnvVarGuard::unset("ARF_R_VERSION");
        let _history_dir = EnvVarGuard::unset("ARF_HISTORY_DIR");

        let help = Cli::generate_help_string(&[]);
        insta::with_settings!({snapshot_path => "../snapshots"}, {
            insta::assert_snapshot!("help_long", help);
        });
    }

    #[test]
    fn test_history_dir_rejects_empty_string() {
        let result = Cli::try_parse_from(["arf", "--history-dir", ""]);
        assert!(result.is_err(), "empty --history-dir should be rejected");
    }

    #[test]
    fn test_no_r_source_overrides_flag_is_available_on_normal_cli() {
        let cli = Cli::try_parse_from(["arf", "--no-r-source-overrides"]).unwrap();
        assert!(cli.no_r_source_overrides);
    }

    #[test]
    fn test_no_r_source_overrides_flag_is_available_on_headless_cli() {
        let cli = Cli::try_parse_from(["arf", "headless", "--no-r-source-overrides"]).unwrap();
        let Some(Commands::Headless(args)) = cli.command else {
            panic!("expected headless command");
        };
        assert!(args.no_r_source_overrides);
    }

    #[test]
    fn test_r_home_subcommand_has_its_own_resolution_flags() {
        let cli = Cli::try_parse_from([
            "arf",
            "r-home",
            "--r-home",
            "/tmp/r-home",
            "--no-r-source-overrides",
            "--config",
            "/tmp/arf.toml",
            "--json",
        ])
        .unwrap();

        let Some(Commands::RHome(args)) = cli.command else {
            panic!("expected r-home command");
        };

        assert_eq!(args.config, Some(PathBuf::from("/tmp/arf.toml")));
        assert_eq!(args.r_home, Some(PathBuf::from("/tmp/r-home")));
        assert!(args.no_r_source_overrides);
        assert!(args.json);
        assert!(args.r_version.is_none());
    }

    #[test]
    #[serial_test::serial]
    fn test_no_r_source_overrides_does_not_conflict_with_r_home() {
        let _r_home = EnvVarGuard::unset("ARF_R_HOME");
        let _r_version = EnvVarGuard::unset("ARF_R_VERSION");
        let cli =
            Cli::try_parse_from(["arf", "--no-r-source-overrides", "--r-home", "/tmp/r-home"]);
        assert!(cli.is_ok());
    }

    #[test]
    #[serial_test::serial]
    fn test_arf_r_home_has_same_precedence_as_r_home() {
        let _r_home = EnvVarGuard::unset("ARF_R_HOME");
        let _r_version = EnvVarGuard::unset("ARF_R_VERSION");
        let _env = EnvVarGuard::set("ARF_R_HOME", "/env/r-home");
        let cli = Cli::try_parse_from(["arf"]).unwrap();

        assert_eq!(
            cli.r_home.as_deref(),
            Some(std::path::Path::new("/env/r-home"))
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_arf_r_version_has_same_precedence_as_with_r_version() {
        let _r_home = EnvVarGuard::unset("ARF_R_HOME");
        let _r_version = EnvVarGuard::unset("ARF_R_VERSION");
        let _env = EnvVarGuard::set("ARF_R_VERSION", "4.5");
        let cli = Cli::try_parse_from(["arf"]).unwrap();

        assert_eq!(cli.r_version.as_deref(), Some("4.5"));
    }

    #[test]
    #[serial_test::serial]
    fn test_cli_value_wins_over_r_source_environment_value() {
        let _r_home = EnvVarGuard::unset("ARF_R_HOME");
        let _r_version = EnvVarGuard::unset("ARF_R_VERSION");
        {
            let _home_env = EnvVarGuard::set("ARF_R_HOME", "/env/r-home");
            let home_cli = Cli::try_parse_from(["arf", "--r-home", "/cli/r-home"]).unwrap();
            assert_eq!(
                home_cli.r_home.as_deref(),
                Some(std::path::Path::new("/cli/r-home"))
            );
        }

        {
            let _version_env = EnvVarGuard::set("ARF_R_VERSION", "4.4");
            let version_cli = Cli::try_parse_from(["arf", "--with-r-version", "4.5"]).unwrap();
            assert_eq!(version_cli.r_version.as_deref(), Some("4.5"));
        }
    }

    #[test]
    #[serial_test::serial]
    fn test_arf_r_home_conflicts_with_with_r_version() {
        let _r_home = EnvVarGuard::unset("ARF_R_HOME");
        let _r_version = EnvVarGuard::unset("ARF_R_VERSION");
        let _env = EnvVarGuard::set("ARF_R_HOME", "/env/r-home");
        let result = Cli::try_parse_from(["arf", "--with-r-version", "4.5"]);

        assert!(result.is_err());
    }

    #[test]
    #[serial_test::serial]
    fn test_headless_r_source_flags_read_environment_values() {
        let _r_home = EnvVarGuard::set("ARF_R_HOME", "/env/r-home");
        let _r_version = EnvVarGuard::unset("ARF_R_VERSION");
        let cli = Cli::try_parse_from(["arf", "headless"]).unwrap();
        let Some(Commands::Headless(args)) = cli.command else {
            panic!("expected headless command");
        };
        assert_eq!(args.r_home, Some(PathBuf::from("/env/r-home")));

        drop(_r_home);
        drop(_r_version);
        let _r_home = EnvVarGuard::unset("ARF_R_HOME");
        let _r_version = EnvVarGuard::set("ARF_R_VERSION", "4.5");
        let cli = Cli::try_parse_from(["arf", "r-home"]).unwrap();
        let Some(Commands::RHome(args)) = cli.command else {
            panic!("expected r-home command");
        };
        assert_eq!(args.r_version.as_deref(), Some("4.5"));
    }

    #[test]
    #[serial_test::serial]
    fn test_headless_r_source_flags_conflict_with_environment_values() {
        let _r_home = EnvVarGuard::set("ARF_R_HOME", "/env/r-home");
        let _r_version = EnvVarGuard::unset("ARF_R_VERSION");
        assert!(Cli::try_parse_from(["arf", "headless", "--with-r-version", "4.5"]).is_err());

        let _r_home = EnvVarGuard::unset("ARF_R_HOME");
        let _r_version = EnvVarGuard::set("ARF_R_VERSION", "4.5");
        assert!(Cli::try_parse_from(["arf", "r-home", "--r-home", "/cli"]).is_err());
    }

    #[test]
    #[serial_test::serial]
    fn test_headless_history_dir_reads_environment_value() {
        let _history_dir = EnvVarGuard::set("ARF_HISTORY_DIR", "/env/history");
        let cli = Cli::try_parse_from(["arf", "headless"]).unwrap();
        let Some(Commands::Headless(args)) = cli.command else {
            panic!("expected headless command");
        };
        assert_eq!(args.history_dir, Some(PathBuf::from("/env/history")));
    }
}
