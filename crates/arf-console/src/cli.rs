//! Command-line interface definition using clap.

use crate::external::rig;
use clap::builder::PossibleValuesParser;
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
    #[arg(value_hint = ValueHint::FilePath)]
    pub script: Option<PathBuf>,

    /// Evaluate R expression and exit
    #[arg(short = 'e', long = "eval")]
    pub eval: Option<String>,

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

    /// [R] Don't use readline (no-op)
    #[arg(long = "no-readline", hide = true)]
    pub no_readline: bool,

    /// [R] Don't restore history (no-op)
    #[arg(long = "no-restore-history", hide = true)]
    pub no_restore_history: bool,

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
    #[arg(long = "history-dir", value_hint = ValueHint::DirPath, hide = true)]
    pub history_dir: Option<PathBuf>,

    /// Disable history (no history saved or loaded)
    ///
    /// Config: history.disabled
    #[arg(long = "no-history", hide = true)]
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
        /// Defaults: radian=~/.radian_history, r=.Rhistory, arf=XDG data dir
        #[arg(long, value_hint = ValueHint::FilePath)]
        file: Option<PathBuf>,

        /// Override hostname for imported entries.
        /// Marks entries to distinguish them from native arf history
        #[arg(long, value_hint = ValueHint::Hostname)]
        hostname: Option<String>,

        /// Perform a dry run without actually importing
        #[arg(long)]
        dry_run: bool,
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

    /// Generate R initialization arguments based on CLI flags.
    ///
    /// Returns a vector of R arguments like ["--quiet", "--no-save", "--no-restore"].
    pub fn r_args(&self) -> Vec<String> {
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
            // Individual flags
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

            if self.restore_data {
                args.push("--restore-data".to_string());
            } else {
                args.push("--no-restore-data".to_string());
            }
        }

        // Always interactive (Unix only - Windows uses Rstart.r_interactive)
        #[cfg(unix)]
        args.push("--interactive".to_string());

        args
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
}
