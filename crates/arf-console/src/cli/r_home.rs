use clap::{Args, ValueHint};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub(crate) struct RHomeArgs {
    /// Path to configuration file
    #[arg(short, long, value_hint = ValueHint::FilePath)]
    pub(crate) config: Option<PathBuf>,

    /// Highest-priority R source: use this R version via rig
    #[arg(long = "with-r-version", conflicts_with = "r_home")]
    pub(crate) r_version: Option<String>,

    /// Highest-priority R source: use this explicit R_HOME path
    #[arg(long = "r-home", value_hint = ValueHint::DirPath, conflicts_with = "r_version")]
    pub(crate) r_home: Option<PathBuf>,

    /// Disable experimental directory-level R source overrides
    #[arg(long = "no-r-source-overrides")]
    pub(crate) no_r_source_overrides: bool,

    /// Print resolution details as JSON
    #[arg(long)]
    pub(crate) json: bool,
}
