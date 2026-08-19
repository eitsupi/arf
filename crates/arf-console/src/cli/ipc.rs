use clap::{Args, Subcommand};

#[derive(Args, Debug)]
pub(crate) struct IpcArgs {
    #[command(subcommand)]
    pub(crate) action: IpcAction,
}

#[derive(Subcommand, Debug)]
pub(crate) enum IpcAction {
    /// List active arf sessions as JSON
    ///
    /// Returns a JSON object with a `sessions` array. Each entry contains
    /// pid, r_version, r_home, socket_path, cwd, started_at, session_type,
    /// log_file, and history_session_id.
    /// Returns `{"sessions": []}` when no sessions are running (exit 0).
    #[command(after_long_help = "\
Examples:
  List all sessions:
    $ arf ipc list

  Extract PIDs with jq:
    $ arf ipc list | jq '.sessions[].pid'")]
    List,
    /// Evaluate R code and return captured output as JSON
    ///
    /// Returns a JSON object with stdout, stderr, value, and error fields.
    /// All fields are always present (null when not applicable). In silent
    /// mode (the default), the printed result appears in value rather than
    /// stdout. R evaluation errors are included in the error field with
    /// exit code 0 — they are a normal response, not an IPC failure.
    #[command(after_long_help = "\
Examples:
  Silent evaluation only runs allowlisted calls, so start the server with the
  ones these examples use:
    $ arf headless --ipc-eval-allow-function '+' \\
        --ipc-eval-allow-function 'Sys.sleep' \\
        --ipc-eval-allow-function 'getwd' &

  Evaluate an expression:
    $ arf ipc eval '1 + 1'

  Pipe code via stdin:
    $ echo '1 + 1' | arf ipc eval

  Bound the wait for a reply without cancelling the R evaluation:
    $ arf ipc eval --timeout 10000 'Sys.sleep(5); 42'

  Run it where the session shows it, which is why it needs no allowlist entry:
    $ arf ipc eval --visible 'cat(\"hello\\n\")'

  Target a specific session when multiple are running:
    $ arf ipc eval --pid 12345 'getwd()'

  Extract the value with jq:
    $ arf ipc eval '1 + 1' | jq -r '.value'")]
    Eval {
        /// R code to evaluate (reads from stdin if omitted)
        code: Option<String>,
        /// PID of the target arf session (optional if only one session is running)
        #[arg(long)]
        pid: Option<u32>,
        /// Also show output in the session (REPL or headless stdout)
        #[arg(long)]
        visible: bool,
        /// Timeout in milliseconds for waiting for the response (default: 300000 = 5 minutes).
        /// This does NOT cancel the R evaluation — long-running code keeps R busy after timeout.
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Send code as user input to a running session
    ///
    /// Unlike `eval`, the code is executed as if the user typed it at the
    /// prompt. Output goes to the session's output streams (the REPL
    /// terminal or headless stdout/log file) and is not captured in the
    /// IPC response. Returns JSON `{"accepted": true}` on success.
    #[command(after_long_help = "\
Examples:
  Send code that appears in the session output:
    $ arf ipc send 'library(dplyr)'

  Pipe code via stdin:
    $ echo 'library(dplyr)' | arf ipc send

  Target a specific session:
    $ arf ipc send --pid 12345 'print(mtcars)'")]
    Send {
        /// R code to send (reads from stdin if omitted)
        code: Option<String>,
        /// PID of the target arf session (optional if only one session is running)
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Get session information as JSON (arf + R environment)
    ///
    /// Returns structured session information including arf version, OS,
    /// log file path (if any), and R environment details (loaded namespaces,
    /// attached packages, library paths, locale, working directory, etc.).
    ///
    /// Unlike `eval "sessionInfo()"`, this returns machine-readable JSON
    /// that can be piped to jq or consumed by AI agents without parsing
    /// human-readable text output. When R is busy, arf-side information is
    /// still returned with `r` set to null and `r_unavailable_reason`
    /// / `hint` fields explaining the situation. The `log_file` field is
    /// null when no dedicated log file is configured (logging to stderr
    /// without redirection). The JSON shape is always consistent — check
    /// `r` for null to determine availability.
    ///
    /// Output is pretty-printed when writing to a terminal, and compact
    /// JSON when piped to another program.
    #[command(after_long_help = "\
Examples:
  Get session info (pretty-printed on terminal):
    $ arf ipc session

  Extract R version with jq:
    $ arf ipc session | jq -r '.r.version'

  Check loaded namespaces:
    $ arf ipc session | jq '.r.loaded_namespaces'")]
    Session {
        /// PID of the target arf session (optional if only one session is running)
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Shut down a running arf headless session (returns JSON `{"accepted": true}`)
    #[command(after_long_help = "\
Examples:
  Shut down the only running session:
    $ arf ipc shutdown

  Shut down a specific session:
    $ arf ipc shutdown --pid 12345")]
    Shutdown {
        /// PID of the target arf session (optional if only one session is running)
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Query command history from a running session
    ///
    /// Returns history entries as JSON, newest first. Output is
    /// pretty-printed when writing to a terminal and compact when piped.
    /// Only completed commands are recorded; a currently executing
    /// command will not appear until it finishes.
    #[command(after_long_help = "\
Examples:
  Show recent history (default 50 entries):
    $ arf ipc history

  Show last 10 entries:
    $ arf ipc history --limit 10

  Search for commands containing 'dplyr':
    $ arf ipc history --grep dplyr

  Filter by working directory:
    $ arf ipc history --cwd /path/to/project

  Show entries since a date:
    $ arf ipc history --since 2026-03-29

  Include history from all sessions (not just current):
    $ arf ipc history --all-sessions

  Combine filters:
    $ arf ipc history --grep 'library' --limit 20

  Extract commands with jq:
    $ arf ipc history | jq -r '.entries[].command'")]
    History {
        /// Maximum number of entries to return (must be positive)
        #[arg(long, default_value = "50", value_parser = clap::value_parser!(i64).range(1..))]
        limit: i64,
        /// Include entries from all sessions, not just the current one
        #[arg(long)]
        all_sessions: bool,
        /// Filter entries by exact working directory
        #[arg(long)]
        cwd: Option<String>,
        /// Filter entries whose command contains this substring
        #[arg(long)]
        grep: Option<String>,
        /// Only return entries after this timestamp (RFC 3339 or YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,
        /// PID of the target arf session (optional if only one session is running)
        #[arg(long)]
        pid: Option<u32>,
    },
}
