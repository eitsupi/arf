use clap::{Args, ValueHint};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub(crate) struct HeadlessArgs {
    #[command(flatten)]
    pub(crate) r_source: super::shared::RSourceArgs,

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

    #[command(flatten)]
    pub(crate) r_compat: super::shared::RCompatArgs,

    #[command(flatten)]
    pub(crate) history: super::shared::HistoryOptions,
}
