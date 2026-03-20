//! Command-line interface definition using clap.

use crate::external::rig;
use clap::builder::PossibleValuesParser;
use clap::builder::TypedValueParser;
use clap::{CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::{Shell, generate};
use std::io;
use std::path::PathBuf;

/// A cross-platform R console written in Rust.
#[derive(Parser, Debug)]
#[command(name = "arf")]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    /// R script file to execute (non-interactive mode)
    #[arg(value_hint = ValueHint::FilePath, conflicts_with = "file")]
    pub script: Option<PathBuf>,

    /// Evaluate R expression and exit
    #[arg(short = 'e', long = "eval")]
    pub eval: Option<String>,

    /// [R] Take input from FILE (same as positional SCRIPT argument)
    #[arg(short = 'f', long = "file", value_hint = ValueHint::FilePath, conflicts_with = "script", hide_short_help = true)]
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

    /// R version to use via rig (overrides r_source config)
    ///
    /// Requires rig to be installed. Use "default" for rig's default,
    /// or specify a version like "4.5" or "release".
    #[arg(long = "with-r-version", conflicts_with = "r_home")]
    pub r_version: Option<String>,

    /// Explicit R_HOME path (overrides r_source config)
    ///
    /// Use this to specify a specific R installation directory.
    /// Mutually exclusive with --with-r-version.
    #[arg(long = "r-home", value_hint = ValueHint::DirPath, conflicts_with = "r_version")]
    pub r_home: Option<PathBuf>,

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
    #[arg(long = "with-ipc")]
    pub with_ipc: bool,

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
    Completions {
        /// The shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Configuration management
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// History management
    History {
        #[command(subcommand)]
        action: HistoryAction,
    },
    /// Interact with a running arf session via IPC
    Ipc {
        #[command(subcommand)]
        action: IpcAction,
    },
    /// Run R with IPC server only (no interactive REPL)
    ///
    /// Starts R and an IPC server without the interactive console.
    /// Useful for AI agents that only need IPC access, or for
    /// CI environments where a terminal is not available.
    /// Exit with Ctrl+C or `arf ipc shutdown`.
    Headless {
        /// Path to configuration file
        #[arg(short, long, value_hint = ValueHint::FilePath)]
        config: Option<PathBuf>,

        /// R version to use via rig (overrides r_source config)
        #[arg(long = "with-r-version", conflicts_with = "r_home")]
        r_version: Option<String>,

        /// Explicit R_HOME path (overrides r_source config)
        #[arg(long = "r-home", value_hint = ValueHint::DirPath, conflicts_with = "r_version")]
        r_home: Option<PathBuf>,

        /// Bind IPC socket to a specific path instead of the default
        ///
        /// Unix: filesystem path (e.g. /tmp/my-arf.sock)
        /// Windows: named pipe path (e.g. \\.\pipe\my-arf)
        // NOTE: FilePath is not ideal on Windows (named pipes aren't filesystem
        // paths), but using cfg_attr to vary the hint per platform would cause
        // shell completion snapshots to differ across machines.
        #[arg(long, value_hint = ValueHint::FilePath)]
        bind: Option<String>,

        /// Write server PID to a file (removed on shutdown)
        #[arg(long = "pid-file", value_hint = ValueHint::FilePath)]
        pid_file: Option<PathBuf>,

        /// Suppress status messages on stderr (IPC path, ready, shutdown)
        #[arg(long)]
        quiet: bool,

        /// Redirect log output to a file instead of stderr
        #[arg(long = "log-file", value_hint = ValueHint::FilePath)]
        log_file: Option<PathBuf>,

        /// Start R in vanilla mode (no init files, no save/restore)
        #[arg(long = "vanilla")]
        vanilla: bool,

        /// [R] Don't read the site and user environment files
        #[arg(long = "no-environ", hide_short_help = true)]
        no_environ: bool,

        /// [R] Don't read the site-wide Rprofile
        #[arg(long = "no-site-file", hide_short_help = true)]
        no_site_file: bool,

        /// [R] Don't read the user's .Rprofile
        #[arg(long = "no-init-file", hide_short_help = true)]
        no_init_file: bool,

        /// [R] Set max number of connections to N
        #[arg(long = "max-connections", hide = true)]
        max_connections: Option<u32>,

        /// [R] Set max size of protect stack to N
        #[arg(long = "max-ppsize", hide = true)]
        max_ppsize: Option<u32>,

        /// [R] Set min number of fixed size obj's ("cons cells") to N
        #[arg(long = "min-nsize", hide = true)]
        min_nsize: Option<String>,

        /// [R] Set vector heap minimum to N bytes; '4M' = 4 MegaB
        #[arg(long = "min-vsize", hide = true)]
        min_vsize: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum IpcAction {
    /// List active arf sessions
    List,
    /// Evaluate R code in a running session (output captured, not shown in REPL)
    Eval {
        /// R code to evaluate
        code: String,
        /// PID of the target arf session (optional if only one session is running)
        #[arg(long)]
        pid: Option<u32>,
        /// Also show output in the REPL terminal
        #[arg(long)]
        visible: bool,
        /// Timeout in milliseconds (default: 300000 = 5 minutes)
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Send code as user input to a running session (shown in REPL)
    Send {
        /// R code to send
        code: String,
        /// PID of the target arf session (optional if only one session is running)
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Show status of a running arf session
    Status {
        /// PID of the target arf session (optional if only one session is running)
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Shut down a running arf headless session
    Shutdown {
        /// PID of the target arf session (optional if only one session is running)
        #[arg(long)]
        pid: Option<u32>,
    },
}

#[derive(Subcommand, Debug)]
pub enum HistoryAction {
    /// Display history database schema and example R code
    Schema,
    /// Import history from another source (experimental)
    ///
    /// Import command history from radian, R's native .Rhistory, or another arf database.
    /// This is an experimental feature and the format may change in future versions.
    Import {
        /// Source format to import from
        #[arg(long, value_enum)]
        from: ImportSource,

        /// Path to the history file/database to import.
        /// Defaults: radian=~/.radian_history, r=.Rhistory, arf=history.dir/r.db
        #[arg(long, value_hint = ValueHint::FilePath)]
        file: Option<PathBuf>,

        /// Override hostname for imported entries.
        /// Marks entries to distinguish them from native arf history
        #[arg(long)]
        hostname: Option<String>,

        /// Perform a dry run without actually importing
        #[arg(long)]
        dry_run: bool,

        /// Import duplicate entries instead of skipping them.
        /// By default, entries that already exist in the target database
        /// are skipped (anti-join on command text and timestamp).
        #[arg(long)]
        import_duplicates: bool,

        /// Force unified export file mode (imports both R and shell history).
        ///
        /// By default, the file format is auto-detected by filename:
        ///   - 'r.db' or 'shell.db' → single-database mode (one history type)
        ///   - Other names (e.g., 'backup.db') → unified mode (both history types)
        ///
        /// Use this flag to force unified mode even for files named r.db/shell.db.
        #[arg(long)]
        unified: bool,

        /// Table name for R history when importing from unified export file
        #[arg(long, default_value = "r")]
        r_table: String,

        /// Table name for shell history when importing from unified export file
        #[arg(long, default_value = "shell")]
        shell_table: String,
    },
    /// Export history to a unified SQLite file (experimental)
    ///
    /// Export both R and shell history to a single SQLite file.
    /// This can be used as a backup or to transfer history between machines.
    Export {
        /// Path to the output SQLite file
        #[arg(long, value_hint = ValueHint::FilePath)]
        file: PathBuf,

        /// Table name for R history in the output file
        #[arg(long, default_value = "r")]
        r_table: String,

        /// Table name for shell history in the output file
        #[arg(long, default_value = "shell")]
        shell_table: String,
    },
}

/// Source format for history import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ImportSource {
    /// radian history file (~/.radian_history)
    Radian,
    /// R native history file (.Rhistory)
    R,
    /// Another arf SQLite history database
    Arf,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Generate a default configuration file
    Init {
        /// Overwrite existing configuration file
        #[arg(long, short)]
        force: bool,
    },
    /// Validate the configuration file
    ///
    /// Check the config file for syntax errors and report any issues.
    /// Exit code 0 means valid, non-zero means file not found or has errors.
    Check {
        /// Path to configuration file to check (defaults to XDG config location)
        #[arg(short, long, value_hint = ValueHint::FilePath)]
        config: Option<PathBuf>,
    },
}

/// Builder for R initialization arguments, shared between REPL and headless modes.
///
/// Headless mode always uses `--no-save --no-restore-data` (save/restore are
/// only meaningful for interactive sessions), while REPL mode supports
/// `--save` and `--restore`.
pub struct RArgsBuilder<'a> {
    pub vanilla: bool,
    pub no_environ: bool,
    pub no_site_file: bool,
    pub no_init_file: bool,
    /// Use `--save` instead of `--no-save`. Only relevant for REPL mode.
    pub save: bool,
    /// Use `--restore` instead of `--no-restore-data`. Only relevant for REPL mode.
    pub restore: bool,
    pub max_connections: Option<u32>,
    pub max_ppsize: Option<u32>,
    pub min_nsize: Option<&'a str>,
    pub min_vsize: Option<&'a str>,
}

impl RArgsBuilder<'_> {
    /// Build the R initialization arguments vector.
    pub fn build(&self) -> Vec<String> {
        let mut args = Vec::new();

        // Always add --quiet (we handle our own banner)
        args.push("--quiet".to_string());

        // --vanilla combines: --no-environ --no-site-file --no-init-file --no-save --no-restore
        if self.vanilla {
            args.push("--no-environ".to_string());
            args.push("--no-site-file".to_string());
            args.push("--no-init-file".to_string());
            args.push("--no-save".to_string());
            args.push("--no-restore-data".to_string());
        } else {
            if self.no_environ {
                args.push("--no-environ".to_string());
            }
            if self.no_site_file {
                args.push("--no-site-file".to_string());
            }
            if self.no_init_file {
                args.push("--no-init-file".to_string());
            }

            // Save/restore flags
            // Default behavior is --no-save --no-restore (like radian)
            if self.save {
                args.push("--save".to_string());
            } else {
                args.push("--no-save".to_string());
            }

            if self.restore {
                args.push("--restore".to_string());
            } else {
                args.push("--no-restore-data".to_string());
            }
        }

        // Memory tuning flags - forward to R
        if let Some(n) = self.max_connections {
            args.push(format!("--max-connections={n}"));
        }
        if let Some(n) = self.max_ppsize {
            args.push(format!("--max-ppsize={n}"));
        }
        if let Some(n) = self.min_nsize {
            args.push(format!("--min-nsize={n}"));
        }
        if let Some(n) = self.min_vsize {
            args.push(format!("--min-vsize={n}"));
        }

        // Always interactive (Unix only - Windows uses Rstart.r_interactive)
        #[cfg(unix)]
        args.push("--interactive".to_string());

        args
    }
}

impl Cli {
    /// Print shell completions to stdout.
    ///
    /// If rig is available, this will include completion values for `--with-r-version`
    /// based on installed R versions.
    ///
    /// TODO: Migrate to dynamic completions using clap_complete's CompleteEnv
    /// when it stabilizes, so completions are generated at TAB-press time
    /// rather than requiring regeneration after installing new R versions.
    pub fn print_completions(shell: Shell) {
        let mut cmd = Cli::command();

        // Inject R version completions from rig if available
        if let Some(possible_values) = Self::get_r_version_completions() {
            // Leak memory for 'static lifetime - acceptable since completions run once and exit
            let leaked: &'static [String] = Box::leak(possible_values.into_boxed_slice());
            let refs: Vec<&'static str> = leaked.iter().map(|s| s.as_str()).collect();
            cmd = cmd.mut_arg("r_version", |arg| {
                arg.value_parser(PossibleValuesParser::new(refs))
            });
        }

        generate(shell, &mut cmd, "arf", &mut io::stdout());
    }

    /// Get possible R version values from rig for shell completion.
    ///
    /// Returns None if rig is unavailable or has no versions installed.
    fn get_r_version_completions() -> Option<Vec<String>> {
        if !rig::rig_available() {
            return None;
        }

        let versions = rig::list_versions().ok()?;
        if versions.is_empty() {
            return None;
        }

        let mut values = vec!["default".to_string()];

        for v in &versions {
            // Add version name (e.g., "4.5.2")
            values.push(v.name.clone());
            // Add aliases (e.g., "release", "oldrel")
            for alias in &v.aliases {
                if !values.contains(alias) {
                    values.push(alias.clone());
                }
            }
        }

        Some(values)
    }

    /// Returns the script file path from either `-f`/`--file` or the positional argument.
    pub fn script_file(&self) -> Option<&PathBuf> {
        self.script.as_ref().or(self.file.as_ref())
    }

    /// Generate R initialization arguments based on CLI flags.
    ///
    /// Returns a vector of R arguments like ["--quiet", "--no-save", "--no-restore"].
    pub fn r_args(&self) -> Vec<String> {
        RArgsBuilder {
            vanilla: self.vanilla,
            no_environ: self.no_environ,
            no_site_file: self.no_site_file,
            no_init_file: self.no_init_file,
            save: self.save,
            restore: self.restore_data || self.restore,
            max_connections: self.max_connections,
            max_ppsize: self.max_ppsize,
            min_nsize: self.min_nsize.as_deref(),
            min_vsize: self.min_vsize.as_deref(),
        }
        .build()
    }

    /// Generate shell completions as a string for testing.
    #[cfg(test)]
    fn generate_completions_string(shell: Shell) -> String {
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        generate(shell, &mut cmd, "arf", &mut buf);
        String::from_utf8(buf).expect("Completions should be valid UTF-8")
    }

    /// Generate help output for a subcommand path for testing.
    #[cfg(test)]
    fn generate_help_string(subcommand_path: &[&str]) -> String {
        let mut cmd = Cli::command();
        for &name in subcommand_path {
            cmd = cmd
                .find_subcommand(name)
                .expect("Subcommand not found")
                .clone();
        }
        cmd.render_long_help().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_completions_bash_snapshot() {
        let completions = Cli::generate_completions_string(Shell::Bash);
        insta::assert_snapshot!("completions_bash", completions);
    }

    #[test]
    fn test_completions_zsh_snapshot() {
        let completions = Cli::generate_completions_string(Shell::Zsh);
        insta::assert_snapshot!("completions_zsh", completions);
    }

    #[test]
    fn test_completions_fish_snapshot() {
        let completions = Cli::generate_completions_string(Shell::Fish);
        insta::assert_snapshot!("completions_fish", completions);
    }

    #[test]
    fn test_completions_powershell_snapshot() {
        let completions = Cli::generate_completions_string(Shell::PowerShell);
        insta::assert_snapshot!("completions_powershell", completions);
    }

    #[test]
    fn test_help_history_import_snapshot() {
        let help = Cli::generate_help_string(&["history", "import"]);
        insta::assert_snapshot!("help_history_import", help);
    }

    #[test]
    fn test_help_history_export_snapshot() {
        let help = Cli::generate_help_string(&["history", "export"]);
        insta::assert_snapshot!("help_history_export", help);
    }

    #[test]
    fn test_help_long_snapshot() {
        let help = Cli::generate_help_string(&[]);
        insta::assert_snapshot!("help_long", help);
    }

    #[test]
    fn test_history_dir_rejects_empty_string() {
        let result = Cli::try_parse_from(["arf", "--history-dir", ""]);
        assert!(result.is_err(), "empty --history-dir should be rejected");
    }
}
