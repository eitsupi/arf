use clap::builder::TypedValueParser;
use clap::{Args, ValueHint};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub(crate) struct HeadlessArgs {
    /// Path to configuration file
    #[arg(short, long, value_hint = ValueHint::FilePath)]
    pub(crate) config: Option<PathBuf>,

    /// Highest-priority R source: use this R version via rig
    ///
    /// Accepts a rig alias (e.g. "release"), "default", a rig-assigned
    /// name, a full version ("4.4.1"), a partial version ("4.4" or "4",
    /// matching the latest release in that series), or a version range in
    /// the style Cargo and npm use ("^4.4", ">=4.3, <5.0").
    ///
    /// Requires rig. Candidates are limited to R versions rig has
    /// installed (from `rig list --json`); the version string is never
    /// passed to rig.
    ///
    /// Takes precedence over ARF_R_VERSION, r_source_overrides and
    /// startup.r_source, which are not consulted at all when this is set.
    ///
    /// Config: startup.r_source
    #[arg(long = "with-r-version", conflicts_with = "r_home")]
    pub(crate) r_version: Option<String>,

    /// Highest-priority R source: use this explicit R_HOME path
    ///
    /// Mutually exclusive with --with-r-version and ARF_R_VERSION.
    ///
    /// Takes precedence over ARF_R_HOME, r_source_overrides and
    /// startup.r_source, which are not consulted at all when this is set.
    ///
    /// Config: startup.r_source
    #[arg(long = "r-home", value_hint = ValueHint::DirPath, conflicts_with = "r_version")]
    pub(crate) r_home: Option<PathBuf>,

    /// Disable experimental directory-level R source overrides
    ///
    /// This only disables r_source_overrides. An R source given by
    /// --r-home, --with-r-version, ARF_R_HOME or ARF_R_VERSION still applies.
    ///
    /// Config: [experimental].r_source_overrides
    #[arg(long = "no-r-source-overrides")]
    pub(crate) no_r_source_overrides: bool,

    /// Bind IPC socket to a specific path instead of the default.
    /// On Unix, ensure the parent directory is user-private (mode 0700)
    /// to avoid a brief permission window before the socket is restricted.
    ///
    /// Unix: filesystem path (e.g. /tmp/my-arf.sock)
    /// Windows: named pipe path (e.g. \\.\pipe\my-arf)
    // NOTE: FilePath is not ideal on Windows (named pipes aren't filesystem
    // paths), but using cfg_attr to vary the hint per platform would cause
    // shell completion snapshots to differ across machines.
    #[arg(long = "ipc-bind", value_hint = ValueHint::FilePath)]
    pub(crate) bind: Option<String>,

    /// Write server PID to a file (removed on shutdown)
    #[arg(long = "ipc-pid-file", value_hint = ValueHint::FilePath)]
    pub(crate) pid_file: Option<PathBuf>,

    /// Suppress status messages on stderr (IPC path, ready, shutdown)
    #[arg(long)]
    pub(crate) quiet: bool,

    /// Print session info as JSON to stdout when ready
    ///
    /// Outputs a JSON object with pid, socket_path, r_version, etc.
    /// Implies --quiet (suppresses status messages on stderr).
    /// Pretty-printed when stdout is a terminal, compact when piped.
    #[arg(long)]
    pub(crate) json: bool,

    /// Redirect log output to a file instead of stderr
    #[arg(long = "log-file", value_hint = ValueHint::FilePath)]
    pub(crate) log_file: Option<PathBuf>,

    /// Start R in vanilla mode (no init files, no save/restore)
    #[arg(long = "vanilla")]
    pub(crate) vanilla: bool,

    /// [R] Don't read the site and user environment files
    #[arg(long = "no-environ", hide_short_help = true)]
    pub(crate) no_environ: bool,

    /// [R] Don't read the site-wide Rprofile
    #[arg(long = "no-site-file", hide_short_help = true)]
    pub(crate) no_site_file: bool,

    /// [R] Don't read the user's .Rprofile
    #[arg(long = "no-init-file", hide_short_help = true)]
    pub(crate) no_init_file: bool,

    /// [R] Set max number of connections to N
    #[arg(long = "max-connections", hide = true)]
    pub(crate) max_connections: Option<u32>,

    /// [R] Set max size of protect stack to N
    #[arg(long = "max-ppsize", hide = true)]
    pub(crate) max_ppsize: Option<u32>,

    /// Custom history directory (overrides default XDG location)
    ///
    /// History will be stored at `{dir}/r.db`.
    ///
    /// Config: history.dir
    #[arg(
        long = "history-dir",
        value_hint = ValueHint::DirPath,
        env = "ARF_HISTORY_DIR",
        hide_short_help = true,
        value_parser = clap::builder::NonEmptyStringValueParser::new().map(PathBuf::from),
    )]
    pub(crate) history_dir: Option<PathBuf>,

    /// Disable history (no history saved)
    ///
    /// Config: history.disabled
    #[arg(long = "no-history", hide_short_help = true)]
    pub(crate) no_history: bool,

    /// [R] Set min number of fixed size obj's ("cons cells") to N
    #[arg(long = "min-nsize", hide = true)]
    pub(crate) min_nsize: Option<String>,

    /// [R] Set vector heap minimum to N bytes; '4M' = 4 MegaB
    #[arg(long = "min-vsize", hide = true)]
    pub(crate) min_vsize: Option<String>,
}
