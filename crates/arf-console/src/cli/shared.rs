use clap::builder::TypedValueParser;
use clap::{Args, ValueHint};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct RSourceArgs {
    /// Path to configuration file
    #[arg(short, long, value_hint = ValueHint::FilePath)]
    pub config: Option<PathBuf>,

    /// Highest-priority R source: use this explicit R_HOME path
    ///
    /// Mutually exclusive with --with-r-version and ARF_R_VERSION.
    ///
    /// Takes precedence over ARF_R_HOME, r_source_overrides and
    /// startup.r_source, which are not consulted at all when this is set.
    ///
    /// Config: startup.r_source
    #[arg(
        long = "r-home",
        value_hint = ValueHint::DirPath,
        env = "ARF_R_HOME",
        conflicts_with = "r_version"
    )]
    pub r_home: Option<PathBuf>,

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

    /// Disable experimental directory-level R source overrides
    ///
    /// This only disables r_source_overrides. An R source given by
    /// --r-home, --with-r-version, ARF_R_HOME or ARF_R_VERSION still applies.
    ///
    /// Config: [experimental].r_source_overrides
    #[arg(long = "no-r-source-overrides")]
    pub no_r_source_overrides: bool,
}

#[derive(Args, Debug)]
pub struct RCompatArgs {
    /// Start R in vanilla mode (no init files, no save/restore)
    #[arg(long = "vanilla")]
    pub vanilla: bool,

    /// [R] Don't read the site and user environment files
    #[arg(long = "no-environ", hide_short_help = true)]
    pub no_environ: bool,

    /// [R] Don't read the site-wide Rprofile
    #[arg(long = "no-site-file", hide_short_help = true)]
    pub no_site_file: bool,

    /// [R] Don't read the user's .Rprofile
    #[arg(long = "no-init-file", hide_short_help = true)]
    pub no_init_file: bool,

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
}

#[derive(Args, Debug)]
pub struct HistoryOptions {
    /// Custom history directory (overrides default XDG location)
    ///
    /// R history is stored at `{dir}/r.db`. The interactive console also
    /// stores shell history at `{dir}/shell.db`.
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
}
