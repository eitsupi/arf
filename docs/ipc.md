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

Headless mode starts R with an IPC server but without the interactive REPL. This is ideal for environments where no terminal is available.

```sh
arf headless
```

The server runs until interrupted by `Ctrl+C`, `SIGTERM`, or `arf ipc shutdown`.

### Options

| Option | Description |
|--------|-------------|
| `--json` | Print session info as JSON to stdout when ready (implies `--quiet`) |
| `--bind <PATH>` | Custom socket path (Unix) or named pipe path (Windows) |
| `--pid-file <PATH>` | Write PID to file (removed on shutdown) |
| `--log-file <PATH>` | Redirect log output to file instead of stderr |
| `--quiet` | Suppress status messages on stderr |
| `--config <PATH>` | Path to configuration file |
| `--with-r-version <VER>` | R version to use via rig |
| `--r-home <PATH>` | Explicit R_HOME path |
| `--vanilla` | Start R without init files |

### JSON Output (`--json`)

When `--json` is specified, arf prints session connection info to stdout as a single JSON object once the server is ready:

```json
{
  "pid": 12345,
  "socket_path": "/home/user/.cache/arf/sessions/12345.sock",
  "r_version": "4.4.1",
  "cwd": "/workspace",
  "started_at": "2026-03-22T10:00:00+09:00",
  "log_file": null,
  "warnings": []
}
```

All keys are always present. `r_version` and `log_file` may be `null`. `warnings` captures non-fatal startup issues (e.g., config parse errors) that would otherwise only appear on stderr.

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

### `arf ipc eval` — Evaluate R Code

Evaluates R code and returns the captured output. The code runs silently by default — output is not shown in the session.

```sh
# Basic evaluation
arf ipc eval '1 + 1'

# With timeout (milliseconds)
arf ipc eval --timeout 10000 'Sys.sleep(5); 42'

# Also show output in the session (REPL or headless stdout)
arf ipc eval --visible 'cat("hello\n")'

# Target a specific session
arf ipc eval --pid 12345 'getwd()'
```

**Parameters:**

| Parameter | Description |
|-----------|-------------|
| `<CODE>` | R code to evaluate (required) |
| `--visible` | Also show output in the session |
| `--timeout <MS>` | Timeout in milliseconds (default: 300000 = 5 minutes) |
| `--pid <PID>` | Target session PID |

**Output format:** stdout contains the captured R output (value + printed output). stderr contains error messages if the evaluation fails. Exit code is non-zero on R errors.

### `arf ipc send` — Send User Input

Sends code as if the user typed it at the prompt. Output goes to the session's output streams (REPL terminal or headless stdout/log file) and is **not** captured in the IPC response.

```sh
# Send code that appears in the session output
arf ipc send 'library(dplyr)'

# Target a specific session
arf ipc send --pid 12345 'print(mtcars)'
```

**When to use `eval` vs `send`:**

| | `eval` | `send` |
|---|---|---|
| Output captured in response | Yes | No |
| Shown in session | Only with `--visible` | Always |
| Timeout support | Yes | No |
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
  "arf_version": "0.2.6-alpha.1",
  "pid": 12345,
  "os": "linux",
  "arch": "x86_64",
  "socket_path": "/home/user/.cache/arf/sessions/12345.sock",
  "started_at": "2026-03-22T10:00:00+09:00",
  "log_file": null,
  "r": {
    "version": "4.4.1",
    "platform": "x86_64-pc-linux-gnu",
    "locale": "en_US.UTF-8",
    "cwd": "/workspace",
    "loaded_namespaces": ["base", "stats", "utils", ...],
    "attached_packages": ["base", "datasets", ...],
    "lib_paths": ["/usr/lib/R/library"]
  },
  "r_unavailable_reason": null,
  "hint": null
}
```

### `arf ipc list` — List Active Sessions

Shows all running arf sessions with IPC enabled.

```sh
arf ipc list
```

### `arf ipc status` — Show Session Status

Displays human-readable status of a running session (PID, R version, socket path, etc.).

```sh
arf ipc status
arf ipc status --pid 12345
```

### `arf ipc shutdown` — Shut Down Headless Session

Sends a graceful shutdown request to a headless session. The session cleans up (removes socket, PID file, session file) before exiting.

```sh
arf ipc shutdown
arf ipc shutdown --pid 12345
```

## IPC in Interactive REPL

You can enable IPC in the interactive REPL without headless mode:

### Using the `--with-ipc` Flag

```sh
arf --with-ipc
```

This starts the REPL normally and also starts the IPC server. External tools can then interact with your session while you continue working interactively.

### Using Meta Commands

Within a running session, you can start and stop the IPC server:

```
:ipc start    # Start the IPC server
:ipc stop     # Stop the IPC server
:ipc status   # Show server status
```

### Interactive + IPC: Mutual Exclusion

When both a human and an external tool use the same session, arf prevents conflicts:

- If you are typing when an IPC `eval` or `send` request arrives, the request is rejected with a `USER_IS_TYPING` error
- If R is busy (not at the prompt), `evaluate` requests are rejected immediately with `R_BUSY`
- If R is not at the prompt, `user_input` / `send` requests are rejected with `R_NOT_AT_PROMPT`
- Clients are expected to handle these errors by retrying later (for example, with backoff); the server does not queue multiple IPC requests beyond a single pending one
- The `session` method always succeeds — it returns arf-side info even when R is busy or not at the prompt

## Transport & Security

### Unix (Linux/macOS)

The IPC server listens on a Unix domain socket. The default path is inside the OS cache directory:

- **Linux**: `~/.cache/arf/sessions/<PID>.sock`
- **macOS**: `~/Library/Caches/arf/sessions/<PID>.sock`

The sessions directory, socket file, and session metadata file are all created with restrictive permissions:
- Directory: mode `0700` (owner only)
- Socket file (`<PID>.sock`): mode `0600` (owner only)
- Session metadata JSON file (`<PID>.json`): mode `0600` (owner only)

This prevents other users on the system from discovering or connecting to your session.

### Windows

The IPC server listens on a named pipe:

```
\\.\pipe\arf-ipc-<PID>
```

### Custom Bind Path

Use `--bind` to specify a custom socket/pipe path:

```sh
# Unix
arf headless --bind /tmp/my-arf.sock

# Windows
arf headless --bind \\.\pipe\my-arf
```

### Session Discovery

Each arf session with IPC enabled writes a session file to the OS cache directory (e.g., `~/.cache/arf/sessions/<PID>.json` on Linux, `~/Library/Caches/arf/sessions/<PID>.json` on macOS). The `arf ipc` client commands use these files to discover running sessions. Stale session files (where the process is no longer running) are automatically cleaned up.

## JSON-RPC Protocol

For tool developers who want to communicate with arf directly (without the `arf ipc` CLI), the server speaks JSON-RPC 2.0 over HTTP on the Unix socket or named pipe.

The server also accepts raw JSON bodies (without HTTP framing) for simpler clients.

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
| `shutdown` | *(none)* | Shut down headless session |

### Response Examples

**Successful evaluation:**

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "stdout": "",
    "stderr": "",
    "value": "[1] 2"
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
    "error": "object 'x' not found"
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

The `evaluate` method captures R output through two separate channels:

- **`stdout` / `stderr` (console output)**: Captured via R's `WriteConsoleEx` callback at the C level. This includes text produced by `cat()`, `message()`, `warning()`, and other writes to the R console.
- **`value` / `error` (structured result)**: Captured separately as the evaluated result (or error) of the last expression, using a binary protocol (`charToRaw()` + `writeBin()` to a temp file, then read from Rust). In silent `evaluate` calls, printed values of expressions appear here rather than in `stdout`/`stderr`.

The result fields in the JSON response are already properly escaped strings — tool developers do not need to handle the raw binary protocol themselves.

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

## Troubleshooting

### "No active arf sessions found"

The `arf ipc` client could not find a session file in the cache directory. This means:

- No arf session has IPC enabled, or
- The session file could not be created (for example, the cache directory is missing or not writable), or
- The session file was cleaned up (the process exited)

**Fix:** Start arf with `arf headless` or `arf --with-ipc`.

### "R is busy"

The R interpreter is executing code and cannot accept new requests. The request is rejected immediately — it is not queued.

**Fix:** Wait for the current operation to complete. For programmatic use, handle `R_BUSY` responses by retrying the request with backoff. Note that `--timeout` only limits how long an evaluation may run — it does not wait for R to become idle.

### "User is typing" (interactive mode)

In interactive REPL mode, IPC requests are rejected when the user has text in the editor buffer.

**Fix:** Clear the input line or press Enter before sending the IPC request. This protection prevents IPC from disrupting the user's typing.

### Connection refused / timeout

The socket exists but the server is not responding.

**Possible causes:**
- The arf process crashed but the socket file was not cleaned up
- R is stuck in an infinite loop or blocking operation

**Fix:** Check if the process is still running with `arf ipc list`. If the session is stale, remove the socket and session files manually from the cache directory (e.g., `~/.cache/arf/sessions/` on Linux).

### Permission denied on socket

On Unix, the socket directory and files are created with restrictive permissions (mode `0700`/`0600`). If you see permission errors:

- Ensure you are connecting as the same user who started arf
- Check that `~/.cache/arf/sessions/` has the correct ownership
