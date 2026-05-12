<div align="center">

# 🐶 arf console

*Alternative R Frontend*

[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/eitsupi/arf)

</div>

<br>

**arf** is a cross-platform R console written in Rust.

> [!WARNING]
> arf is under active development. The configuration file format and history file format are not yet stable and may change without notice in future versions.

<div align="center">

![arf demo](demo/arf.gif)

</div>

## Features

- Single binary with no runtime dependencies
- Cross-platform: Linux, macOS, and Windows
- [rig](https://github.com/r-lib/rig) integration — switch R versions with `--with-r-version` or `:switch` within a session
- Vi and Emacs editing modes
- Multiline editing with proper indentation
- Auto-matching brackets and quotes (with smart skip-over)
- Tab completion for R objects, functions, and file paths inside strings
- fzf-style history search with `Ctrl+R`; import from radian or `.Rhistory`
- Customizable keyboard shortcuts (`Alt+-` → ` <- `, `Alt+P` → ` |> `)
- Command status indicator (shows error symbol when previous command failed)
- Fuzzy help browser with `:help` or `:h` — search across all installed packages
- Tree-sitter based syntax highlighting with customizable colors
- Reprex mode with optional auto-formatting via [Air](https://github.com/posit-dev/air)
- Shell mode (`:shell` to enter, `:r` to return)
- Configurable prompts and colors with placeholders (`{version}`, `{cwd}`, `{status}`)
- SQLite-backed persistent history with import/export support
- IPC server for AI agent and CI integration
- Headless mode for non-interactive environments (CI, background jobs)

## Installation

### Pre-built Binaries

Pre-built binaries are available from [GitHub Releases](https://github.com/eitsupi/arf/releases).

#### Shell Installer (Linux/macOS)

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/eitsupi/arf/releases/latest/download/arf-console-installer.sh | sh
```

#### winget (Windows)

```sh
winget install --id eitsupi.arf
```

#### Scoop (Windows)

```sh
scoop bucket add r-bucket https://github.com/cderv/r-bucket.git
scoop install r-bucket/arf
```

#### cargo-binstall

```sh
cargo binstall --git https://github.com/eitsupi/arf arf-console
```

#### Manual Download

Download the archive for your platform from [GitHub Releases](https://github.com/eitsupi/arf/releases) and extract the binary to a directory in your `PATH`.

### Third-Party Packages

[![Packaging status](https://repology.org/badge/vertical-allrepos/arf.svg)](https://repology.org/project/arf/versions)

#### Homebrew (macOS/Linux)

```sh
brew install arf
```

#### AUR (Arch Linux/Manjaro)

```sh
yay -S arf-bin
# or use paru
paru -S arf-bin
```

### Build from Source

```sh
cargo install --git https://github.com/eitsupi/arf.git
```

> [!NOTE]
> **For Windows**: A fix for VT input handling (bracketed paste, etc.) is pending in an upstream dependency
> ([crossterm#1030](https://github.com/crossterm-rs/crossterm/pull/1030)).
>
> So installing from crates.io (`cargo install arf-console`) is **not recommended**.

## Quick Start

```sh
# Launch arf
arf

# Use a specific R version (requires rig)
arf --with-r-version 4.4

# Enable reprex mode for reproducible examples
arf --reprex
```

### Interactive Help

Press `:h` or `:help` to open the fuzzy help browser:

```
─ Help Search [12345 topics] ─────────────────────────
  Filter: mutate_

 > dplyr::mutate           Create, modify, and delete columns
   dplyr::mutate_all       Mutate multiple columns
   dplyr::mutate_at        Mutate multiple columns
   dplyr::mutate_if        Mutate multiple columns
─────────────────────────────────────────────────────
  ↑↓ navigate  Tab/Enter select  Esc exit
```

## Meta Commands

arf extends R with `:` prefixed meta commands:

| Command | Description |
|---------|-------------|
| `:help`, `:h` | Open interactive help browser |
| `:info`, `:session` | Show session information (version, config path, etc.) |
| `:shell` | Enter shell mode |
| `:r` | Return to R mode |
| `:system <cmd>` | Execute a single shell command inline |
| `:reprex` | Toggle reprex mode |
| `:autoformat`, `:format` | Toggle auto-formatting (requires Air) |
| `:switch <version>` | Restart with different R version (requires rig) |
| `:restart` | Restart R session |
| `:cd [path]` | Change working directory (no args → home) |
| `:pushd <path>` | Push current directory to stack and change to path |
| `:popd` | Pop directory from stack and change to it |
| `:history browse` | Browse and manage command history |
| `:history clear` | Clear command history (supports `r`, `shell`, `all` targets) |
| `:history schema` | Display history database schema and R examples |
| `:commands`, `:cmds` | Show available commands |
| `:quit`, `:exit` | Exit arf |

## Configuration

arf uses a TOML configuration file located at:

- **Linux**: `~/.config/arf/arf.toml`
- **macOS**: `~/Library/Application Support/arf/arf.toml`
- **Windows**: `%APPDATA%\arf\arf.toml`

Generate a default configuration with:

```sh
arf config init
```

### Example Configuration

```toml
[startup]
r_source = "auto"       # "auto", "rig", or { path = "/path/to/R" }
show_banner = true

# Initial mode settings (can be toggled at runtime)
[startup.mode]
reprex = false
autoformat = false      # Requires Air CLI

[editor]
mode = "emacs"          # "emacs" or "vi"
auto_match = true       # Auto-close brackets and quotes
auto_suggestions = "all" # "none", "all", or "cwd"

# Keyboard shortcuts (crokey format)
[editor.key_map]
alt-hyphen = " <- "
alt-p = " |> "          # Use "ctrl-shift-m" for RStudio-style

[prompt]
format = "{status}R {version}> "
continuation = "+  "
shell_format = "[{shell}] $ "
mode_indicator = "prefix"   # "prefix", "suffix", or "none"

# Command status indicator
[prompt.status]
symbol = { error = "✗ " }   # success = "" (empty) by default
override_prompt_color = false

# R runtime configuration
[r]
auto_width = true       # Sync options(width) with terminal size

# Reprex static configuration
[mode.reprex]
comment = "#> "

# Syntax highlighting colors
[colors.r]
keyword = "LightBlue"
string = "Green"
comment = "DarkGray"
number = "LightMagenta"
operator = "Yellow"

[colors.prompt]
main = "LightGreen"
```

See the full [Configuration Guide](docs/configuration.md) for all options.

## Headless Mode & IPC

> [!NOTE]
> This feature is experimental and may change in future versions.

Run R without a terminal and interact via IPC — useful for AI agents, CI, and editor extensions. Unlike MCP-based solutions that require R package installation and an MCP client, arf provides a single binary with a simple CLI for programmatic R access:

```sh
# Start headless R with IPC server
arf headless

# From another terminal, evaluate R code
arf ipc eval '1 + 1'

# Get session info as JSON
arf ipc session | jq '.r.version'

# Shut down when done
arf ipc shutdown
```

See the full [IPC & Headless Mode Guide](docs/ipc.md) for details.

## Experimental Features

Features in this section are under development and may change or be removed in future versions. Configure them under the `[experimental]` section.

### Spinner

Displays an animated spinner at the start of the line while R is evaluating code. **Disabled by default.**

To enable, set the `frames` option:

```toml
[experimental.prompt_spinner]
frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"  # Braille dots
color = "Cyan"              # Spinner color (default: Cyan)
```

See [Spinner configuration](docs/configuration.md#spinner) for all options and frame style examples.

### Auto-completion while typing

Show the completion menu automatically after typing a minimum number of characters, without requiring Tab.

```toml
[experimental]
completion_min_chars = 3  # Show completions after 3 characters
```

When not set, completion requires pressing Tab (the default behavior). This is similar to radian's `complete_while_typing` feature.

### Fuzzy R completion

Use fuzzy matching for R code completions. When enabled, typing `sf::geo` can match `sf::st_geometry` and `library(dpl` can match `dplyr`. **Disabled by default.**

```toml
[experimental.r_completion]
fuzzy = true
```

Both `::` (exported names) and `:::` (internal names) are supported. Package exports are cached per-package with a 5-minute TTL for performance.

The `package_functions` option controls which function calls trigger package-name fuzzy completion (defaults to `["library", "require"]`). Users can add custom functions like `"box::use"`:

```toml
[experimental.r_completion]
fuzzy = true
package_functions = ["library", "require", "box::use"]
```

### History forget

Automatically remove commands that produced errors from history. Similar to fish's [sponge](https://github.com/meaningful-ooo/sponge) plugin.

```toml
[experimental.history_forget]
enabled = true
delay = 2          # Keep last N failed commands for quick retry
on_exit_only = false  # Purge on each prompt (false) or only on exit (true)
```

See [History forget configuration](docs/configuration.md#history-forget) for all options.

### History export/import

> [!CAUTION]
> These features are experimental and have not been thoroughly tested. The format and behavior may change in future versions.

#### Export

Export both R and shell history to a unified SQLite file for backup or transfer:

```sh
# Export to a unified file
arf history export --file ~/arf_backup.db

# Export with custom table names
arf history export --file ~/arf_backup.db --r-table my_r --shell-table my_shell
```

**Options:**

| Option | Description |
|--------|-------------|
| `--file` | Path to output SQLite file (required) |
| `--r-table` | Table name for R history (default: `r`) |
| `--shell-table` | Table name for shell history (default: `shell`) |

#### Import

Import command history from radian, R's native `.Rhistory`, or another arf database:

```sh
# Preview what would be imported (dry run)
arf history import --from radian --dry-run

# Import from radian (default: ~/.radian_history)
arf history import --from radian

# Import from R's native history
arf history import --from r --file .Rhistory

# Import from a unified export file (restores both R and shell history)
arf history import --from arf --file ~/arf_backup.db

# Import from a single-database file (r.db or shell.db)
arf history import --from arf --file /path/to/r.db

# Import with custom hostname (to distinguish from native entries)
arf history import --from radian --hostname "radian-import"
```

**Options:**

| Option | Description |
|--------|-------------|
| `--from` | Source format: `radian`, `r`, or `arf` (required) |
| `--file` | Path to source file (required for `arf`, defaults to standard locations for others) |
| `--hostname` | Custom hostname to mark imported entries |
| `--dry-run` | Preview without importing |
| `--import-duplicates` | Import duplicate entries instead of skipping them |
| `--unified` | Force unified file mode (import both R and shell from table names) |
| `--r-table` | Table name for R history in unified file (default: `r`) |
| `--shell-table` | Table name for shell history in unified file (default: `shell`) |

**Supported sources:**

| Source | Description | Timestamps | Multiline | Mode routing |
|--------|-------------|:----------:|:---------:|:------------:|
| `radian` | `~/.radian_history` | Preserved | Preserved | By `# mode:` |
| `r` | `.Rhistory` or `R_HISTFILE` | - | - | → `r.db` |
| `arf` | SQLite database (`--file` required) | Preserved | Preserved | By filename or `--unified` |

**Mode routing for arf format:**

- Files named `r.db` or `shell.db`: Single-database import (by filename)
- Other filenames or `--unified` flag: Unified import (by table names, imports both R and shell)

**Notes:**

- By default, duplicate entries are skipped during import (matched by command text and timestamp). Use `--import-duplicates` to import all entries regardless.
- Self-import is detected and rejected when importing from an arf database to the same target file.
- **Important:** Exit arf before exporting to ensure the source databases are in a consistent state. The export itself uses atomic writes to prevent incomplete output files, but reading while arf is writing may capture inconsistent data.

## Known Issues

### Error detection uses `options(error = ...)`

arf uses R's `options(error = ...)` to detect errors from packages like dplyr/rlang that output error messages to stdout instead of stderr. This is necessary for accurate error tracking in command history and the status indicator.

**Limitations**:
- If you set a custom error handler via `options(error = ...)`, arf will chain to your handler, but arf's handler takes precedence. Your handler will still be called after arf records the error.
- There is a slight performance overhead (~microseconds) on each prompt due to R API calls for checking and resetting error state. This is negligible in practice but may be noticeable in benchmarks.

## Related Projects

- [radian](https://github.com/randy3k/radian) — A 21st century R console written in Python.

- [sircon](https://github.com/lrberge/sircon) — Simple R Console. A Windows-only R console with powerful autocomplete and a macro language for custom shortcuts. Some of sircon's advanced features are future goals for arf.

## Acknowledgements

arf builds on the following projects:

- **[ark](https://github.com/posit-dev/ark)** — The `arf-libr` and `arf-harp` crates are derived from ark's `libr` and `harp` crates, which provide the foundation for embedding R in Rust applications. Windows initialization follows ark's pattern. [tree-sitter-r](https://github.com/r-lib/tree-sitter-r) powers syntax highlighting and code analysis.

- **[radian](https://github.com/randy3k/radian)** — arf's design and many features are inspired by radian, including shell mode, stderr formatting, and tab completion patterns. The feedback and discussions in radian's GitHub issues over the years have also been invaluable.

- **[felp](https://github.com/atusy/felp)** — The interactive fuzzy help browser concept was inspired by felp's `fuzzyhelp()` function.

- **[reedline](https://github.com/nushell/reedline)** — The line editor library from the Nushell project that powers arf's interactive editing.

- **[mcp-repl](https://github.com/t-kalinowski/mcp-repl)** — The headless mode's pager redirection and graphics device configuration are based on mcp-repl's approach for non-interactive R sessions.

- **[MCPRepl.jl](https://github.com/hexaeder/MCPRepl.jl)** / **[Kaimon.jl](https://github.com/kahliburke/Kaimon.jl)** — The IPC server design (shared REPL with agent code visibility, synchronous evaluation via backend queue) was heavily informed by these Julia MCP server implementations.

## License

MIT
