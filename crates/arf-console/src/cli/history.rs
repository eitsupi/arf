use clap::{Args, Subcommand, ValueHint};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub(crate) struct HistoryArgs {
    #[command(subcommand)]
    pub(crate) action: HistoryAction,
}

#[derive(Subcommand, Debug)]
pub(crate) enum HistoryAction {
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
        /// Defaults: radian=~/.radian_history, r=.Rhistory, arf=persistent history `dir`/r.db
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
pub(crate) enum ImportSource {
    /// radian history file (~/.radian_history)
    Radian,
    /// R native history file (.Rhistory)
    R,
    /// Another arf SQLite history database
    Arf,
}
