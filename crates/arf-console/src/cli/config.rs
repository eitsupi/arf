use clap::{Args, Subcommand, ValueHint};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub(crate) struct ConfigArgs {
    #[command(subcommand)]
    pub(crate) action: ConfigAction,
}

#[derive(Subcommand, Debug)]
pub(crate) enum ConfigAction {
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
