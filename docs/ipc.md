# IPC & Headless Mode

arf includes a built-in IPC (Inter-Process Communication) server that allows external tools to interact with a running R session. This enables AI agents, CI pipelines, and editor extensions to evaluate R code, query session state, and control the session programmatically.

> [!WARNING]
> IPC is an experimental feature. The protocol and CLI interface may change in future versions.

## Overview

The IPC system has two parts:

1. **Server** — Runs inside arf, listening on a Unix socket (Linux/macOS) or named pipe (Windows)
2. **Client** — The `arf ipc` subcommands that connect to a running server

The server can be enabled in three ways:

| Method | Use case |
|--------|----------|
| `arf headless` | CI, AI agents, background R sessions (no terminal needed) |
| `arf --with-ipc` | Interactive REPL with external tool access |
| `:ipc start` meta command | Enable IPC in an already-running session |

## Headless Mode

Headless mode starts R with an IPC server but without a terminal UI. This is ideal for environments where no terminal is available.

```sh
arf headless
```

The server runs until interrupted by `Ctrl+C`, `SIGTERM`, or `arf ipc shutdown`. Immediately after startup there is a brief window where the IPC endpoint is listening but R is not yet ready, so early client requests may return `R_BUSY`. For scripted or automated use, prefer `arf headless --json` and treat the JSON emitted on stdout as the readiness signal, or retry the first `arf ipc` command with a small backoff until it succeeds.

### Headless console limitations

Visible requests (`send` and `eval --visible`) use R's normal REPL. R's parser
determines expression boundaries; a braced block is one expression. R invokes task
callbacks for successfully evaluated top-level expressions, not failed ones,
and applies its normal autoprinting and `.Last.value` semantics.

Leading horizontal whitespace may be omitted at verified top-level input
boundaries to prevent R from treating long whitespace-only fragments as EOF.
Source columns and leading indentation can therefore differ from the submitted
code. Original line breaks and string contents are preserved; no artificial
line breaks are added or expressions rewritten. Values, expression meanings,
and task callback counts are preserved.

Older R versions may retain native REPL limitations for long console input;
this path does not guarantee correct handling of such inputs on R 4.0/4.1.

Standard nested console reads such as `parse()` reading from the console,
`readline()`, menus, and browser prompts receive EOF; further IPC requests
cannot supply console responses. Native console readers without R evaluation
frames are unsupported. R's main and continuation prompt options must be
readable, non-empty, distinct strings and must not resemble `Browse[n]>`
prompts. `--no-echo` is unsupported in headless mode. Invalid or ambiguous
prompt conditions produce a diagnostic and stop the session without consuming
the next request.

### Options

| Option | Description |
|--------|-------------|
| `--json` | Print session info as JSON to stdout when ready (implies `--quiet`) |
| `--ipc-bind <PATH>` | Custom socket path (Unix) or named pipe path (Windows) |
| `--ipc-pid-file <PATH>` | Write PID to file (removed on shutdown) |
| `--log-file <PATH>` | Redirect log output to file instead of stderr |
| `--history-dir <PATH>` | Override history database directory |
| `--no-history` | Keep command history in memory for this session only (no disk load/save) |
| `--ipc-expose <EXPR>` | Expose a named list of R functions to IPC clients at startup |
| `--quiet` | Suppress status messages on stderr |
| `--config <PATH>` | Path to configuration file |
| `--with-r-version <VER>` | R version to use via rig |
| `--r-home <PATH>` | Explicit R_HOME path |
| `--r-provider <PROVIDER>` | Explicitly select the current synced rig project or an active Pixi environment |
| `--no-r-source-overrides` | Disable the configured R source override provider chain |
| `--vanilla` | Start R without init files |

### PID file lifecycle (interactive and headless)

The PID file is the current-process pointer for an integration slot; it is not
a readiness marker. Readiness is established by the IPC session metadata (or
the `--json` output). Interactive `:restart` and `:switch` are a
short transition: clients should use bounded retries, re-read the PID and
session metadata, and detect the new generation by a changed `started_at`
value. On Unix the replacement keeps ownership of a matching PID file. On
Windows the parent relinquishes the file before spawning the replacement, so
the file is briefly absent and then publishes a new PID. On Windows only, the
default endpoint also changes because it contains the child PID; each retry
must resolve it again. On Unix the default endpoint path remains the same
across an exec restart, although it is temporarily unavailable during the
restart window. In both cases, `started_at` changes for the new generation.
An explicitly configured `--ipc-bind` path is reused by the replacement on
both platforms. For an interactive session, a relative custom path is resolved
once against arf's initial working directory, before R startup profiles can
change it. Headless sessions follow the same pointer/readiness contract until
shutdown.

### JSON Output (`--json`)

When `--json` is specified, arf prints session connection info to stdout as a single JSON object once the server is ready:

```json
{
  "pid": 12345,
  "socket_path": "/run/user/1000/arf/12345.sock",
  "r_version": "4.4.2",
  "r_home": "/opt/R/4.4.2/lib/R",
  "cwd": "/workspace",
  "started_at": "2026-03-22T10:00:00+09:00",
  "log_file": null,
  "history_session_id": 1742601600000000000,
  "ipc_policy": {
    "silent": {
      "mode": "restricted",
      "allowed_functions": ["+", "mean"]
    },
    "visible": {
      "mode": "approval_not_required"
    }
  },
  "history_runtime": {
    "state": "persistent",
    "source": "default",
    "path": "/home/user/.local/state/arf/history/r.db",
    "reason": null,
    "detail": null
  },
  "r_source_override": {
    "state": "applied",
    "provider": "toml-key",
    "file": "rproject.toml",
    "key": "project.r_version",
    "requested_version": "4.4",
    "resolved_version": "4.4.2"
  },
  "r_source_selection": {
    "provider": "rig",
    "selected_by": {
      "kind": "version_request",
      "requested_r_home": null,
      "requested_version": "4.4",
      "source": {
        "kind": "project_file",
        "name": null,
        "path": "/project/rproject.toml",
        "format": "toml",
        "key": "project.r_version"
      }
    },
    "metadata": null
  },
  "warnings": []
}
```

All keys are always present. `r_version`, `r_home`, and `log_file` may be `null`; `history_session_id` is `null` only when history initialization is unavailable. `ipc_policy` is always present in this readiness output: `silent` is either restricted with its complete sorted `allowed_functions` list (an empty restricted list still permits bare literals and identifiers) or unrestricted with no allowlist field. Because this is headless readiness output, `visible` has mode `approval_not_required` for both `send` and `eval --visible`. `history_runtime` reports `persistent`, configured `volatile`, fallback `volatile`, or `unavailable`; its `source` is where the history directory came from: `default`, `legacy_default` (the previous default location, used because it holds history and the current default does not; a warning is added to `warnings`), `explicit` (`--history-dir`, `ARF_HISTORY_DIR`, or `history.mode`), or `volatile`; its `path` is a diagnostic path when one was requested (for example, for a persistent or fallback open, or an unavailable initialization), and `detail` contains an optional human-readable failure diagnostic. `r_home` is the R installation the session is using, or `null` when the session has no R. The `r_source_override` object is always present; its state is one of `applied`, `not_configured`, `no_match`, `failed`, `disabled`, or `shadowed_by_cli`, and its other fields are `null` unless an override was applied. `warnings` captures non-fatal startup issues (e.g., config parse errors or history fallback diagnostics) that would otherwise only appear on stderr.

`r_source_selection` reports the actual selected provider, its provenance, and provider `metadata` using the same fields as [`arf r resolve`](r-resolve.md#json-descriptor), for both configured overrides and explicit `--r-provider` or `ARF_R_PROVIDER` requests. Provider details such as the Pixi environment or the rig project manifest appear only there.

The legacy `r_source_override` object reports only the configured override chain. For a Pixi or rig-project override, `provider` is `"pixi"` or `"rig-project"`, and `file`, `key`, and `requested_version` are `null`. `resolved_version` may be `null` for Pixi and is always `null` for rig-project, whose installation name is not treated as an exact R version. An explicit provider request does not mark the override as applied (`shadowed_by_cli`, or `disabled` when overrides were disabled). Pixi warnings use the same `warnings` array and never require an interactive confirmation.

The IPC `r_version` is measured from a live R session; `arf r resolve` reports `resolved_version`, a prediction made before R starts.

Output is pretty-printed when stdout is a terminal, compact when piped. This is useful in CI scripts:

```sh
arf headless --json | jq -r .socket_path
```

### R Configuration in Headless Mode

Headless mode automatically configures R for non-interactive use:

- **Pager**: Redirected to stdout (no interactive `less`)
- **Help**: Forced to plain text (`options(help_type = "text")`)
- **Browser**: Disabled (URLs are printed instead of opening a browser)
- **Graphics**: Defaults to file-based devices (png/pdf) instead of X11
- **Save/Restore**: Always `--no-save --no-restore-data`

## IPC Subcommands

All `arf ipc` subcommands connect to a running arf session. If only one session is active, it is used automatically. When multiple sessions are running, use `--pid` to target a specific one.

### Output Format

All IPC action subcommands (`list`, `eval`, `send`, `session`, `shutdown`, `history`) output JSON to stdout. Output is pretty-printed when stdout is a terminal, compact when piped. Errors are written to stderr as structured JSON:

```json
{
  "error": {
    "code": "R_BUSY",
    "message": "R is busy",
    "hint": "R is executing code. Wait for it to finish, or use 'arf ipc session' to check status.",
    "data": null
  }
}
```

All four fields (`code`, `message`, `hint`, `data`) are always present. `hint` is `null` when no hint is available. `data` contains additional structured information (e.g. `{"buffer": "..."}` for `USER_IS_TYPING`) or `null`.

The `code` field is a string identifier for stable matching. Process exit codes indicate the error category:

| Exit code | Meaning |
|-----------|---------|
| 0 | Success |
| 2 | IPC transport error (socket/pipe connection failed, connection-level read timeout) |
| 3 | Session resolution error (no session found, ambiguous PID) |
| 4 | JSON-RPC protocol error (R busy, user typing, server-side request timeout from `--timeout`, etc.) |

> **Note:** Transport-level timeouts (failing to connect to the socket or low-level read/write timeouts) produce exit code **2** (`TRANSPORT_ERROR`). Server-side evaluation timeouts triggered by `arf ipc eval --timeout` result in a JSON-RPC error response from the server, which the client reports as exit code **4**.

Error code strings:

| Code | Exit | Description |
|------|------|-------------|
| `TRANSPORT_ERROR` | 2 | Socket/pipe connection failed, connection-level read timeout, etc. |
| `SESSION_NOT_FOUND` | 3 | No session with the specified PID, or no sessions at all |
| `SESSION_AMBIGUOUS` | 3 | Multiple sessions running and `--pid` not specified |
| `R_BUSY` | 4 | R is executing code |
| `R_NOT_AT_PROMPT` | 4 | R is in browser/menu mode |
| `INPUT_ALREADY_PENDING` | 4 | Another IPC request is already queued |
| `USER_IS_TYPING` | 4 | User is typing in the REPL (see `data` fields below) |
| `INCOMPLETE_INPUT` | 4 | R code is syntactically incomplete and would enter the continuation prompt |
| `R_EVAL_NOT_ALLOWED` | 4 | Evaluation was rejected by the server-side syntactic policy |
| `INPUT_NOT_APPROVED` | 4 | Interactive `send` was not approved at the REPL prompt |
| `EMPTY_RESPONSE` | 4 | Server returned no result |
| `PARSE_ERROR` | 4 | Invalid JSON in request |
| `INVALID_REQUEST` | 4 | Not a valid JSON-RPC request |
| `METHOD_NOT_FOUND` | 4 | Unknown method name |
| `INVALID_PARAMS` | 4 | Invalid method parameters |
| `INTERNAL_ERROR` | 4 | Server internal error |
| `PROTOCOL_ERROR` | 4 | Other JSON-RPC error |

#### `USER_IS_TYPING` error data

When the user is typing in the REPL, the `data` field contains:

| Field | Type | Description |
|-------|------|-------------|
| `buffer` | string | Current editor buffer content (capped at 1024 characters) |
| `buffer_truncated` | boolean | `true` if the buffer was truncated |
| `buffer_original_length` | integer | Full character count of the original buffer |

### `arf ipc eval` — Evaluate R Code

What decides the policy is whether a human can see the operation, not whether
the session is headless or interactive. Silent eval leaves no trace a person
watching the session would notice, so it is protected by an exact allowlist.
`send` and `eval --visible` do surface in the session, so they are protected by
human approval instead: an interactive session shows a confirmation prompt, and
a headless session has no human to ask, so both headless `send` and headless
`eval --visible` run immediately and are unrestricted.

Headless history records each non-whitespace external IPC code request once it
begins execution, including silent eval. Interactive history records visible
`send` and `eval --visible` requests. Interactive silent eval is omitted from
command history so background tool queries do not appear in the user's command
recall. Requests rejected before execution, requests whose client has stopped
waiting before execution begins, and `session` or `history` queries are not
recorded. A recorded request appears in history immediately; its exit status
remains `null` until a reliable result is available. History metadata such as
the start time and working directory is captured when execution begins.

Plain (silent) IPC evaluation uses a best-effort tree-sitter-r policy in both
interactive and headless sessions. The default allowlist is empty. Configure
exact direct call targets at startup, for example
`--ipc-eval-allow-function mean` or `--ipc-eval-allow-function stats::median`;
nested calls must also be allowlisted. Bare literals and object references
(bare identifiers) evaluate without any configuration. Extraction operators
(`$`, `[`, and `[[`) remain allowlist-gated; enable them with repeated
`--ipc-eval-allow-function` flags, as in this debugger-style inspection setup:

```sh
arf headless \
  --ipc-eval-allow-function '$' \
  --ipc-eval-allow-function '[' \
  --ipc-eval-allow-function '[[' &
```

Other R operators use their exact spelling (such as `+`) in the same list.
Assignments, control flow, computed callees, the native pipe `|>`, and `:::`
are always rejected in restricted mode. Syntax errors and policy violations
are rejected before R evaluation and history recording.
`evaluate` and `user_input` also reject code containing a NUL character before
dispatch in both interactive and headless sessions.
`--ipc-eval-unrestricted` is a startup-only escape hatch. This policy is not an
R sandbox and does not promise that an allowed function is non-mutating. Evaluating
a bare identifier can itself run code by forcing a promise or triggering an active
binding; this is allowed by default because IPC eval never permits assignment, so a
caller cannot create such a binding through IPC eval.

The same policy is advertised as the `ipc_policy` object in the headless
`--json` readiness output and live `arf ipc session` responses. The interactive
`:info` pager reports whether the IPC server is enabled.
When enabled, it shows the silent-eval mode, lists each configured target on
its own line so the complete allowlist can be checked without horizontal
scrolling, and states the current approval requirement for visible requests
(`send`/`eval --visible`). When disabled, it omits the policy details.

Evaluates R code and returns the captured output. The code runs silently by
default — output is not shown in the session. Silent eval uses the direct
evaluation path in both interactive and headless sessions; it does not invoke
the top-level task callback. With `--visible`, an interactive session uses the
same human approval as `send`, while headless evaluation runs immediately.
Visible `eval` and `send` use R's normal REPL in both session types, with the
expression and callback semantics described under [Headless console
limitations](#headless-console-limitations). Visible `eval` captures autoprint
and callback output in `stdout` and evaluation errors in `stderr`, returning
`null` for both `value` and `error`.

```sh
# Basic evaluation
arf headless \
  --ipc-eval-allow-function length \
  --ipc-eval-allow-function Sys.sleep \
  --ipc-eval-allow-function getwd &
arf ipc eval 'length("abc")'

# With timeout (milliseconds; Sys.sleep must be allowlisted at server startup)
arf ipc eval --timeout 10000 'Sys.sleep(5)'

# Also show output in the session (REPL or headless stdout).
# This runs where the session shows it, so it needs no allowlist entry.
arf ipc eval --visible 'cat("hello\n")'

# Target a specific session
arf ipc eval --pid 12345 'getwd()'
```

**Parameters:**

| Parameter | Description |
|-----------|-------------|
| `<CODE>` | R code to evaluate (required) |
| `--visible` | Evaluate through the R REPL and show output in the session. In an interactive session, this is governed by the same human approval as `send`; in headless mode it runs immediately. The response's `value` and `error` are `null`; autoprint and callback output are captured in `stdout`, and evaluation errors in `stderr`. |
| `--timeout <MS>` | Timeout in milliseconds for waiting for the response (default: 300000 = 5 minutes). This does NOT cancel the R evaluation — long-running code keeps R busy after timeout. |
| `--pid <PID>` | Target session PID |

**Output format:** JSON object with `stdout` (string), `stderr` (string), `value` (string or null), `error` (string or null), and `execution_id` (number). These fields are always present. For visible evaluation, `value` and `error` are `null`; normal REPL output and errors appear in `stdout` and `stderr`. Silent evaluation (the default) uses the direct evaluation path, so the printed result appears in `value` and R evaluation errors appear in `error`, with exit code 0 — they are normal responses, not IPC failures.

When the timeout fires, the server returns a JSON-RPC error response instead of a result, and the client prints that error as structured JSON on stderr and exits with code 4 — the timeout is therefore not reported in the result object's `error` field. The R evaluation continues, so the session stays busy until it finishes.

Example (silent eval, result captured in `value`):

```json
{
  "stdout": "",
  "stderr": "",
  "value": "[1] 2",
  "execution_id": 7,
  "error": null
}
```

Example (R error):

```json
{
  "stdout": "",
  "stderr": "",
  "value": null,
  "error": "object 'x' not found",
  "execution_id": 8
}
```

### `arf ipc send` — Send User Input

Sends code through R's normal REPL, as if the user typed it at the prompt. It uses the same expression and callback semantics as visible `eval`. Output goes to the session's output streams (REPL terminal or headless stdout/log file) and is **not** captured in the IPC response.

`send` is a visible, history-recorded operation. In an interactive REPL it is
shown for human approval; in headless mode it runs immediately because there is
no human to ask. It is not governed by the silent-eval allowlist.

In an interactive REPL, `send` displays the complete escaped code that will be
executed after approval. Control and other non-printing characters are escaped
so they cannot alter or spoof the terminal display. The approval block is
shown as three separate lines and requires an explicit `y` or `Y` key for each
request by default:

```text
# [arf] IPC send request:
  <escaped code>
# [arf] Press y to approve, any other key declines:
```

The heading and `# [arf]` prefix are dark cyan, the escaped code is yellow,
and the confirmation text is bold yellow. The three-line structure carries the
meaning independently of color. Long code is not shortened; it remains
available in the terminal scrollback, so the displayed escaped code corresponds
to the complete code being approved.

Any other key, including Enter, `n`, Ctrl+C, Ctrl+D, or Esc, rejects the
request. Non-key terminal events are ignored while waiting. A read error or a
request whose client timeout has elapsed also rejects the request. This is
controlled for the current process only with `:ipc
send-policy allow` and restored with `:ipc send-policy prompt`; it cannot be
changed by configuration or startup CLI options. Headless `send` remains
immediate.

```sh
# Send code that appears in the session output
arf ipc send 'library(dplyr)'

# Target a specific session
arf ipc send --pid 12345 'print(mtcars)'
```

**Output format:** JSON object with `accepted` (bool) and `execution_id` (number). Example: `{"accepted": true, "execution_id": 7}`. `accepted: true` acknowledges that the request was accepted; it does not report whether evaluation succeeded or completed.

**When to use `eval` vs `send`:**

| | `eval` | `send` |
|---|---|---|
| Output captured in response | Yes | No |
| Shown in session | Only with `--visible` | Always |
| User-configurable timeout (`--timeout` / `timeout_ms`) | Yes | No |
| Use case | Programmatic access | Human-visible interaction |

### `arf ipc session` — Get Session Info

Returns structured session information as JSON, including arf version, R environment details, and runtime state.

```sh
# Pretty-printed on terminal, compact when piped
arf ipc session

# Extract R version with jq
arf ipc session | jq -r '.r.version'

# Check loaded namespaces
arf ipc session | jq '.r.loaded_namespaces'
```

The response always has the same shape. When R is busy, the `r` field is `null` and `r_unavailable_reason` explains why:

```json
{
  "arf_version": "0.2.6",
  "pid": 12345,
  "session_id": "9f3c1a07d25b4e68a1c0b7e45d2f8a13",
  "os": "linux",
  "arch": "x86_64",
  "r_home": "/opt/R/4.4.2/lib/R",
  "socket_path": "/run/user/1000/arf/12345.sock",
  "started_at": "2026-03-22T10:00:00+09:00",
  "log_file": null,
  "ipc_policy": {
    "silent": {
      "mode": "restricted",
      "allowed_functions": []
    },
    "visible": {
      "mode": "approval_not_required"
    }
  },
  "r": null,
  "r_unavailable_reason": "R is busy evaluating another expression",
  "hint": null
}
```

All top-level keys are always present. `session_id` is a random identifier generated when the arf process starts. It stays the same across `:ipc stop` and `:ipc start`, so it tells a restarted arf apart from a restarted server. `ipc_policy` is the complete policy object described in
the evaluation section; it is present even when R is busy. `r_home` is the R installation the
session is using, or `null` when the session has no R. The `r` object contains information collected by evaluating R
and may be `null` while R is busy or unavailable. `arch` is the architecture of the arf process,
not the R installation.

The example above shows a headless session, so `visible` has mode
`approval_not_required`; it covers both `send` and `eval --visible`. For an
interactive session, this live policy reflects the current `:ipc send-policy`
state: `allow` reports `approval_not_required`, while `prompt` reports
`approval_required`.

When R is idle, the `r` field contains session details (other top-level fields omitted for brevity):

```json
{
  "r": {
    "version": "4.4.1",
    "platform": "x86_64-pc-linux-gnu",
    "locale": "en_US.UTF-8",
    "cwd": "/workspace",
    "loaded_namespaces": ["base", "stats", "utils"],
    "attached_packages": ["base", "datasets"],
    "lib_paths": ["/usr/lib/R/library"]
  },
  "r_unavailable_reason": null,
  "hint": null
}
```

### `arf ipc list` — List Active Sessions

Returns all running arf sessions with IPC enabled as JSON.

```sh
arf ipc list

# Example output:
# {
#   "sessions": [
#     {
#       "pid": 12345,
#       "r_version": "4.4.1",
#       "r_home": "/opt/R/4.4.1/lib/R",
#       "socket_path": "/run/user/1000/arf/12345.sock",
#       "cwd": "/workspace",
#       "started_at": "2026-03-22T10:00:00+09:00",
#       "session_type": "headless",
#       "log_file": null,
#       "history_session_id": 1742601600000000000
#     }
#   ]
# }
```

When no sessions are running, returns `{"sessions": []}` (exit 0).

The `session_type` field is `"headless"` for sessions started with
`arf headless` and `"interactive"` for sessions running an interactive REPL.
The field is always present in current session metadata.

The `r_home` field is the R installation the session is using. A `null` value
means either that the session has no R, or that it was created by an older arf
that did not record the path: a session file written before this field existed
is listed with `r_home` set to `null`, like any other unset field. Clients must
treat `null` as unknown and must not assume an R installation is available.

`r_version`, `r_home`, `log_file`, and `history_session_id` may be `null`. The
list is discovery metadata only; it does not include the effective IPC policy.
Query `arf ipc session` to obtain the live policy at the time of the request.

### `arf ipc history` — Query Command History

Returns command history entries from the session's SQLite history database as JSON. By default, only entries from the current session are returned. This method is handled on the server thread and does not touch R, so it works even when R is busy.

```sh
# Show recent history from this session (default 50 entries)
arf ipc history

# Show last 10 entries
arf ipc history --limit 10

# Include history from all sessions (not just current)
arf ipc history --all-sessions

# Search for commands containing 'dplyr'
arf ipc history --grep dplyr

# Filter by working directory
arf ipc history --cwd /path/to/project

# Show entries since a date
arf ipc history --since 2026-03-29

# Combine filters
arf ipc history --grep 'library' --limit 20

# Extract commands with jq
arf ipc history | jq -r '.entries[].command'
```

**Parameters:**

| Parameter | Description |
|-----------|-------------|
| `--limit <N>` | Maximum number of entries to return (default: 50, must be positive) |
| `--all-sessions` | Include entries from all sessions, not just the current one |
| `--cwd <PATH>` | Filter entries by exact working directory |
| `--grep <PATTERN>` | Filter entries whose command contains this substring |
| `--since <DATE>` | Only return entries after this timestamp (RFC 3339 or `YYYY-MM-DD`) |
| `--pid <PID>` | Target session PID |

**Output format:** JSON object with `entries` array (newest first) and `session_id`. Each entry contains `command`, `timestamp`, `cwd`, `exit_status`, and `session_id` (all fields are always present; null when not available). Output is pretty-printed when stdout is a terminal, compact when piped.

> [!NOTE]
> A command appears in history as soon as execution begins, so a currently
> running command can appear with a `null` exit status. The status stays `null`
> when arf cannot determine a reliable result.

### `arf ipc shutdown` — Shut Down Headless Session

Requests shutdown of a headless session. The session removes its socket, PID
file, and session file before exiting. As before, IPC shutdown does not invoke
`.Last()`, save the workspace, or run R's cleanup routine.

```sh
arf ipc shutdown
arf ipc shutdown --pid 12345
```

**Output format:** JSON object with `accepted` (bool). Example: `{"accepted": true}`

## IPC in Interactive REPL

You can enable IPC in the interactive REPL without headless mode:

### Using the `--with-ipc` Flag

```sh
arf --with-ipc
```

This starts the REPL normally and also starts the IPC server. External tools can then interact with your session while you continue working interactively.

Pass `--ipc-expose '<R expression>'` with `--with-ipc` to publish handlers at
startup. The same `--ipc-expose` option is available as a subcommand option to
`arf headless`.

### Using Meta Commands

Within a running session, you can start and stop the IPC server:

```
:ipc start    # Start the IPC server
:ipc stop     # Stop the IPC server
:ipc status   # Show server status
```

When handlers have been exposed, `:ipc` or `:ipc status` also shows their API
state and names. `:ipc revoke` permanently revokes them for the session.

### Interactive + IPC: Mutual Exclusion

When both a human and an external tool use the same session, arf prevents conflicts:

- If you are typing when an IPC `eval` or `send` request arrives, the request is rejected with a `USER_IS_TYPING` error
- If an IPC `eval` or `send` request contains syntactically incomplete R code, it is rejected with an `INCOMPLETE_INPUT` error
- If R is busy (not at the prompt), `evaluate` requests are rejected immediately with `R_BUSY`
- If R is not at the prompt, `user_input` / `send` requests are rejected with `R_NOT_AT_PROMPT`
- Clients are expected to handle these errors by retrying later (for example, with backoff). In interactive/REPL mode, the server accepts at most one pending request — additional requests are rejected with `INPUT_ALREADY_PENDING`. In headless mode, requests are queued and processed sequentially
- The `session` and `history` methods do not touch R and can be called even when R is busy or not at the prompt. `session` always succeeds; `history` may fail only when no history owner is available or the store cannot be queried
- `list` reads local session files and does not connect to any server, so it always works regardless of R state

## Transport & Security

### Unix (Linux/macOS)

The IPC server listens on a Unix domain socket. The default path depends on the platform:

- When `$XDG_RUNTIME_DIR` is set: `$XDG_RUNTIME_DIR/arf/<PID>.sock` (typically `/run/user/<UID>/arf/<PID>.sock` on Linux with systemd)
- When `$XDG_RUNTIME_DIR` is not set (e.g. macOS): `<temp_dir>/arf-<random>/<PID>.sock` (where `<temp_dir>` is the system temp directory, e.g. `$TMPDIR`; on Linux this is typically `/tmp`)

The socket directory and file are created with restrictive permissions:
- Socket directory: mode `0700` (owner only)
- Socket file (`<PID>.sock`): mode `0600` (owner only)

Before using the socket directory, arf validates that it is not a symlink, is owned by the current user, and is not writable by group or other users. If validation fails, arf attempts to use a per-process fallback directory instead. If all candidate directories are unsafe or cannot be created, IPC does not start and arf returns an error rather than continuing without IPC.

Session metadata JSON files (`<PID>.json`) are stored separately in the OS cache directory (e.g., `~/.cache/arf/sessions/` on Linux):
- Directory: mode `0700` (owner only)
- File: mode `0600` (owner only)

This prevents other users on the system from discovering or connecting to your session.

### Windows

The IPC server listens on a named pipe:

```
\\.\pipe\arf-ipc-<PID>
```

> [!NOTE]
> Although Windows 10 1803+ supports AF_UNIX sockets, arf currently uses named pipes on Windows because the async runtime (tokio) does not yet support AF_UNIX on Windows. See [tokio#2201](https://github.com/tokio-rs/tokio/issues/2201) for upstream progress.

### Custom Bind Path

Use `--ipc-bind` to specify a custom socket/pipe path. This works for both
headless and interactive REPL (`--with-ipc`) modes:

```sh
# Headless — Unix
arf headless --ipc-bind /tmp/my-arf.sock

# Headless — Windows
arf headless --ipc-bind \\.\pipe\my-arf

# Interactive REPL — Unix (editor launches arf and knows the socket path upfront)
arf --with-ipc --ipc-bind /tmp/my-arf.sock
```

> [!NOTE]
> On Unix, the socket file is created with `bind()` and then restricted to `0600`. There is a brief window between these two calls where the default umask applies. If you use a custom path, ensure the parent directory is user-private (e.g., mode `0700`) to prevent other users from connecting during that window.

### Session Discovery

Each arf session with IPC enabled writes a session file to the OS cache directory (e.g., `~/.cache/arf/sessions/<PID>.json` on Linux, `~/Library/Caches/arf/sessions/<PID>.json` on macOS). The session file contains discovery metadata such as the socket path so that `arf ipc` client commands can discover running sessions; it does not store the effective IPC policy. Query `arf ipc session` for the live policy at request time. Stale session files (where the process is no longer running) are automatically cleaned up.

### Remote Access (No Built-in TCP)

arf intentionally does not listen on TCP. Supporting TCP would require building authentication and encryption into arf itself, which is better handled by dedicated tools. Instead, use an existing proxy or tunnel to expose the local socket remotely:

- **SSH tunneling** (OpenSSH 6.7+ supports Unix socket forwarding) — recommended for most cases, as it provides encryption and authentication with no extra software
- **socat** — lightweight bidirectional relay between Unix sockets and TCP, useful for quick bridging on trusted networks
- **Reverse proxies** (Caddy, nginx) — suitable for persistent setups, especially when TLS is required

On Windows, arf listens on a named pipe which these tools cannot target directly. In WSL environments, [npiperelay](https://github.com/albertony/npiperelay) can bridge a Windows named pipe to stdin/stdout, allowing it to be combined with socat or SSH.

## JSON-RPC Protocol

For tool developers who want to communicate with arf directly (without the `arf ipc` CLI), the server speaks JSON-RPC 2.0 over HTTP on the Unix socket or named pipe.

The server also accepts raw JSON request bodies (without HTTP request-line/headers) for simpler clients. Responses are still sent as standard `HTTP/1.1 200 OK` messages with headers followed by a JSON body, so raw-JSON clients need to strip the HTTP headers before parsing the response.

### Request Format

Send an HTTP POST request with a JSON-RPC body:

```http
POST / HTTP/1.1
Host: localhost
Content-Type: application/json
Content-Length: ...
Connection: close

{"jsonrpc": "2.0", "id": 1, "method": "evaluate", "params": {"code": "1 + 1"}}
```

### Available Methods

| Method | Parameters | Description |
|--------|-----------|-------------|
| `evaluate` | `code` (string), `visible` (bool, default false), `timeout_ms` (int, optional) | Evaluate R code and return captured output |
| `user_input` | `code` (string) | Send code as user input |
| `session` | *(none)* | Get session information |
| `history` | `limit` (int, default 50), `all_sessions` (bool, default false), `cwd` (string, optional), `grep` (string, optional), `since` (string, optional) | Query command history |
| `shutdown` | *(none)* | Shut down the session (headless mode only; returns an error in interactive mode) |
| `attach` | `client` (object, optional), `subscribe` (array of patterns, optional), `protocol` (int, optional) | Turn the connection into a [long-lived connection](#long-lived-connections) (raw newline-delimited JSON only) |
| `subscribe` | `patterns` (array of strings) | Long-lived connections only: add event subscriptions |
| `unsubscribe` | `patterns` (array of strings) | Long-lived connections only: remove event subscriptions |
| `call` | `handler` (string), `args` (JSON, optional), `queue_timeout_ms` (int, optional) | Call a handler exposed by the session; available on single-shot and long-lived connections |

### Response Examples

**Successful evaluation:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "stdout": "",
    "stderr": "",
    "value": "[1] 2",
    "execution_id": 7
  }
}
```

**R error:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "stdout": "",
    "stderr": "",
    "error": "object 'x' not found",
    "execution_id": 8
  }
}
```

Errors are caught by `tryCatch`, so the error message appears in the `error` field (via `conditionMessage()`). The `stderr` field is typically empty for caught errors.

**R is busy:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": {
    "code": -32000,
    "message": "R is busy"
  }
}
```

### Output Capture

Silent `evaluate` calls capture R output through two separate channels. Visible
`evaluate` calls use normal REPL evaluation instead, so their `value` and
`error` fields are `null`, autoprint and callback output are captured in
`stdout`, and evaluation errors are captured in `stderr`. `user_input`/`send` also use normal
REPL evaluation and do not return evaluation results.

- **`stdout` / `stderr` (console output)**: Captured via R's `WriteConsoleEx` callback at the C level. This includes text produced by `cat()`, `message()`, `warning()`, and other writes to the R console.
- **`value` / `error` (structured result)**: For silent `evaluate`, the evaluated result (or error) is captured separately using a binary protocol (`charToRaw()` + `writeBin()` to a temp file, then read from Rust). Printed values appear here rather than in `stdout`/`stderr`. Visible evaluation uses normal REPL output and leaves both fields `null`.

The result fields in the JSON response are already properly escaped strings — tool developers do not need to handle the raw binary protocol themselves.

The results of `evaluate`, `user_input`, and `call` include an `execution_id`.
It identifies the execution within this arf process, independently of the
JSON-RPC request ID. `user_input` acknowledges acceptance with
`{"accepted":true,"execution_id":7}`; it does not wait for completion.
Errors after execution starts also include `data.execution_id` when available.
A reply timeout limits waiting for a response; it does not end the execution or
its event context. Use the [completion event](#execution-completion) to observe
when user code actually finishes.

### Long-lived Connections

A client that wants to receive events, or to keep one connection for many requests, opens the connection with `attach`. The request must be raw JSON (not HTTP) and must end with a newline:

```json
{"jsonrpc":"2.0","id":1,"method":"attach","params":{"client":{"name":"my-tool","version":"0.1"},"subscribe":["demo.*"],"protocol":1}}
```

All `params` are optional. `client` is informational only and is never used for authentication or authorization. `subscribe` lists the initial [subscription patterns](#events). `protocol` is the version of the line protocol described here; it defaults to 1, and any other value is refused. If `attach` is refused (unsupported protocol, an invalid pattern, or more subscriptions than a connection may hold), the server sends an error response and closes the connection. An `attach` sent over HTTP is refused with `INVALID_REQUEST`, and an `attach` without an `id` closes the connection without a response. If the first line does not end right after the request, the request is treated as an ordinary single-shot request instead.

The response is:

```json
{"jsonrpc":"2.0","id":1,"result":{"connection_id":3,"api_version":1,"capabilities":["events","execution_events"],"session_id":"9f3c1a07d25b4e68a1c0b7e45d2f8a13","session":{"...":"as returned by the session method"}}}
```

`session` is the same object the `session` method returns, but it never waits for R: `r` is `null` and `r_unavailable_reason` says so. Call `session` to get R's details. `session_id` identifies the arf process and is the same after the server is restarted within it.

After the response, each line in either direction is one JSON-RPC 2.0 message:

- A line longer than 1 MiB, or a line that is not valid JSON, closes the connection. Empty lines are ignored. Valid JSON that is not a valid JSON-RPC request is answered with an `INVALID_REQUEST` error (with the `id` if it has one).
- Requests without an `id` are notifications: they get no response and are not carried out.
- Every method listed above can be used, with the same parameters, results and errors as on a single-shot connection, plus `subscribe` and `unsubscribe`. A second `attach` is answered with an error, and the connection stays open.
- Requests run concurrently and their responses arrive in completion order, so match them by `id`. At most 32 requests may be outstanding; further ones are answered immediately with `R_BUSY` until one finishes. `subscribe` and `unsubscribe` are applied in the order they arrive.
- Clients must keep their write side open for the life of the connection. When the server sees end-of-file it cancels outstanding requests and closes the connection, so responses after a half-close are not guaranteed.
- When the connection ends, requests that R has not started yet are cancelled. A request that R is already running continues, but its result is discarded.
- The server closes a connection to which it cannot write a line within 30 seconds.
- `shutdown` behaves as on a single-shot connection; the shutdown begins after its response has been written, or after the write has failed or timed out.

#### Events

`subscribe` and `unsubscribe` take `patterns`, a list of topic names or `prefix.*` wildcards (`prefix.*` matches every topic starting with `prefix.`, not `prefix` itself). A topic is dot-separated; each segment is made of ASCII letters, digits, `_` and `-`, and the whole name is at most 128 bytes. All patterns in a request are validated first, and the request is applied entirely or not at all. A connection may hold at most 64 subscriptions. The results are `{"added": n}` and `{"removed": n}`, counting patterns that were not subscribed before and patterns that were, respectively.

Matching events arrive as notifications:

```json
{"jsonrpc":"2.0","method":"event","params":{"seq":42,"topic":"demo.finished","data":{},"execution":null}}
```

`execution` is `null` unless the event belongs to an execution. Events on one connection arrive in increasing `seq` order. `seq` exists only to compare order: it is shared by all events of the arf process, so gaps are normal and do not mean that events were lost. No event is sent before the `attach` response.

Events are never dropped silently. A connection that cannot keep up, or that may have missed an event because the server's own queue overflowed, is disconnected instead. After such a disconnect, attach again and fetch the state you need again. To avoid missing changes at the start, attach with `subscribe` first and only then fetch the initial state: an event may then describe something the fetched state already contains, but nothing is lost unless the connection is disconnected.

#### Execution completion

Subscribe to `arf.execution_completed` (or `arf.*`) to observe completed user
code in interactive and headless sessions:

```json
{"jsonrpc":"2.0","method":"event","params":{"seq":43,"topic":"arf.execution_completed","data":{"id":7,"origin":3,"kind":"user_code","visible":false,"status":"ok"},"execution":7}}
```

`data.id` matches the response's `execution_id`. `origin` is `"terminal"` for
terminal input or the originating IPC connection's numeric ID (including
single-shot connections). `kind` is `"user_code"`; `visible` distinguishes
normal REPL execution from silent evaluation. `status` is `"ok"`, `"error"`, or
`"unknown"` when a reliable outcome is unavailable.

One completion event covers one submitted terminal input or one `evaluate` /
`user_input` request, even if it contains multiple expressions. Silent evaluations
also produce completion events. Initial prompts and returns to input without
any evaluation do not. Handler `call` executions have IDs and their R-published
events carry those IDs, but they do not produce completion events, allowing
subscribers to fetch workspace state without triggering a refresh loop.

Clients refreshing state on completion should use handler `call` rather than
`evaluate`, which emits its own completion event, or use `origin` to recognize
their own executions.

Events published during an execution carry its ID in `execution` and have
smaller `seq` values than its completion event. A reply timeout does not trigger
completion: the event is published once, when the running user code finishes.

#### Exposing and calling R handlers

Start arf with `--with-ipc --ipc-expose '<R expression>'` in the interactive REPL, or `arf headless --ipc-expose '<R expression>'` in a headless session, to expose handlers. The expression runs after R startup profiles and before the IPC server accepts requests. It must return a named list whose values are functions. Each name is a selector in `<namespace>/<method>` form; the `arf/` namespace is reserved. For example:

```sh
arf headless --ipc-expose 'list("demo/add" = function(args) list(sum = sum(args$x)))'
```

The complete list is checked before any handler becomes available. An evaluation or validation error aborts startup with a diagnostic. The functions are kept alive for the session.

Use the `call` method to invoke one exposed selector with JSON arguments:

```json
{"jsonrpc":"2.0","id":2,"method":"call","params":{"handler":"demo/add","args":{"x":[2,3]}}}
```

If `args` is omitted, the handler receives `NULL`. The response result contains `value` (converted to JSON), `stdout`, `stderr`, and `execution_id`. Calls run on the R thread and their console output is captured. Calls do not enter R's command history. They are not subject to the code evaluation policy or `send` approval because the client selects a published function and supplies data rather than R source code. A handler can still perform arbitrary side effects, so exposing one is not a read-only or safe operation. Publish only handlers whose arguments are validated; never use client values to select files, evaluate source, or construct executable code.

Call requests from all clients share one FIFO queue with a limit of 64 waiting calls and 8 MiB of queued handler names and compact JSON arguments. A full queue is rejected immediately. `queue_timeout_ms` sets how long a call may wait before it starts (default 30,000 ms; allowed range 1–300,000 ms). A call that expires before starting is not run. Closing an attached connection also drops a call that has not started; an already running R function continues and its response is discarded if the client has disconnected. Single-shot connections preserve write-half-close compatibility: a disconnect while waiting is not detected until the response is written, so use an attached connection when cancellation on disconnect is required. Calls do not interrupt one another once running.

Errors distinguish an unpublished or revoked handler (`-32010`), a full queue (`-32011`), a queue timeout (`-32012`), argument conversion (`-32013`), an R error (`-32014`), return conversion (`-32015`), and an interrupt (`-32016`). R errors include the condition message and class; errors include captured `stdout` and `stderr` when available. An error does not roll back side effects or trigger a retry.

`session` reports `handler_api.state` as `unexposed`, `locked`, or `revoked`, along with the available handler names. `:ipc status` (or `:ipc`) displays the same state and `:ipc revoke` permanently revokes all handlers for that session. Revocation also prevents queued calls from starting, and cannot stop a handler that is already running. In the interactive REPL, at most one queued call starts per idle poll; calls can run while the user is editing a line without changing the input buffer or input history. Calls wait while R is evaluating or the editor is in an alternate mode, and may expire before starting. A locked handler list reveals its names and interface to every IPC client. The function's enclosing environment, bindings, and S3/S4 methods can change later, so the published list does not freeze the function's behavior. The lock prevents changing which selectors are published; it does not sandbox their code or environment.

##### Publishing events from R

R code running in the session can publish events to subscribed connections with two `.Call` routines, registered under `PACKAGE = "(embedding)"`:

- `arf_publish_event(topic, data)` takes exactly two arguments. `topic` is a non-`NA` character string. `data` is any value that converts to JSON under the strict rules below, and `NULL` becomes JSON `null`. It returns `TRUE` when the event was queued on a running IPC server and `FALSE` when there is nowhere to publish it (no IPC server is running). `TRUE` is not a delivery guarantee: an event nobody is subscribed to is dropped.
- `arf_api_version()` takes no arguments and returns `1L`.

Publishing never waits: the event is queued and the IPC server delivers it, so subscribers receive it while R keeps computing. Anything wrong with the call is an R error, whether or not a server is running: a topic that is not a valid single string or that starts with the reserved `arf.` prefix (topic rules are those of [Events](#events)), a `data` value that cannot be converted (the message names the offending element by JSON Pointer), an event larger than 1 MiB (the topic and a fixed allowance for the notification envelope count towards it), or an overflow of the server's shared event queue. An overflow also disconnects the connections whose subscriptions may match the dropped event. `execution` identifies the active user-code or handler execution, and is `null` when there is no active execution.

R values convert to JSON as follows: `NULL` is `null`; a length-1 logical, integer, double or character vector is a boolean, number or string; an unnamed list is an array; a named list is an object. Longer atomic vectors, `NA`, `NaN`, infinite numbers, objects with a `class` attribute (factors, dates and data frames included), duplicate or `NA` names, and every other type are errors. Wrap vectors in `as.list()` to send them as arrays.

Detect the routines, and call them through a small wrapper:

```r
arf_publish <- function(topic, data = NULL) {
  invisible(.Call("arf_publish_event", topic, data, PACKAGE = "(embedding)"))
}

has_arf_events <- function() {
  is.loaded("arf_api_version", PACKAGE = "(embedding)", type = ".Call") &&
    identical(.Call("arf_api_version", PACKAGE = "(embedding)"), 1L)
}
```

The check detects the interface for feature detection only; it is not authentication. On Windows the routines exist before startup profiles run. On Unix they are registered only after R has finished starting, so `.Rprofile` and `Rprofile.site` cannot use them: detect lazily, on first use rather than at load time, and do not cache a `FALSE` obtained during startup.

### Error Codes

| Code | Name | Description |
|------|------|-------------|
| -32700 | Parse Error | Invalid JSON |
| -32600 | Invalid Request | Not a valid JSON-RPC request |
| -32601 | Method Not Found | Unknown method name |
| -32602 | Invalid Params | Invalid method parameters |
| -32603 | Internal Error | Server internal error |
| -32000 | R Busy | R is executing code |
| -32001 | R Not At Prompt | R has not returned to the prompt |
| -32002 | Input Already Pending | Another IPC request is already queued |
| -32003 | User Is Typing | User is typing in the REPL (interactive mode only) |
| -32004 | Incomplete Input | R code is syntactically incomplete |

## Troubleshooting

### "No active arf sessions found"

The `arf ipc` client could not find a session file in the cache directory. This means:

- No arf session has IPC enabled, or
- The session file could not be created (for example, the cache directory is missing or not writable), or
- The session file was cleaned up (the process exited)

**Fix:** Start arf with `arf headless` or `arf --with-ipc`.

### "R is busy"

In interactive/REPL mode, the request is rejected immediately with `R_BUSY` — it is not queued. In headless mode, requests are queued and processed sequentially; clients will typically block until the current operation finishes or their own timeout elapses.

**Fix:** For interactive mode, handle `R_BUSY` responses by retrying the request with backoff. In headless mode, configure appropriate client-side timeouts. Note that `--timeout` only limits how long the IPC call waits for a reply — it does not cancel the underlying R evaluation, and long-running code may keep R busy even after the client times out.

### "User is typing" (interactive mode)

In interactive REPL mode, IPC requests are rejected when the user has text in the editor buffer.

**Fix:** Clear the input line or press Enter before sending the IPC request. This protection prevents IPC from disrupting the user's typing.

### Connection refused / timeout

The socket exists but the server is not responding.

**Possible causes:**
- The arf process crashed but the socket file was not cleaned up
- R is stuck in an infinite loop or blocking operation

**Fix:** Check if the process is still running with `arf ipc list`. If the session is stale, remove the socket file shown in the `socket_path` field (for example, `$XDG_RUNTIME_DIR/arf/<PID>.sock` or `<temp_dir>/arf-<random>/<PID>.sock` on Unix) and the session metadata file (`~/.cache/arf/sessions/<PID>.json`).

### Permission denied on socket

On Unix, the socket directory and files are created with restrictive permissions (mode `0700`/`0600`). If you see permission errors:

- Ensure you are connecting as the same user who started arf
- Check that the socket directory (`$XDG_RUNTIME_DIR/arf/` or `<temp_dir>/arf-<random>/`) has the correct ownership
