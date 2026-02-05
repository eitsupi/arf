<div align="center">

# 🐶 arf console

*Alternative R Frontend*

[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/eitsupi/arf)

</div>

<br>

**arf** is a modern, cross-platform R console written in Rust. It provides a rich interactive experience with fuzzy help search, intelligent history navigation, and syntax highlighting—all with fast startup times.

> [!WARNING]
> arf is under active development. The configuration file format and history file format are not yet stable and may change without notice in future versions.

<div align="center">

![arf demo](demo/arf.gif)

</div>

## Highlights

- **Single Binary, Zero Dependencies** — One small executable with no runtime dependencies. Just download and run.

- **rig Integration** — Seamless [rig](https://github.com/r-lib/rig) (R Installation Manager) support. Switch R versions with `--with-r-version` flag, or use the `:switch` meta command to change versions within a running session.

- **Fuzzy History Search** — fzf-style history search with `Ctrl+R`. Type fragments to find past commands quickly. Import your existing history from radian or R's `.Rhistory`.

- **Syntax Highlighting** — Tree-sitter based highlighting for R code with customizable colors.

- **Interactive Help Browser** — Fuzzy search through R documentation with `:help` or `:h`. Find any function across all installed packages instantly.

## Features

- Cross-platform: Linux, macOS, and Windows
- Vi and Emacs editing modes
- Multiline editing with proper indentation
- Auto-matching brackets and quotes (with smart skip-over)
- Tab completion for R objects, functions, and file paths inside strings
- Customizable keyboard shortcuts (`Alt+-` → ` <- `, `Alt+P` → ` |> `)
- Command status indicator (shows error symbol when previous command failed)
- Reprex mode with optional auto-formatting via [Air](https://github.com/posit-dev/air)
- Shell mode (`:shell` to enter, `:r` to return)
- Configurable prompts and colors with placeholders (`{version}`, `{cwd}`, `{status}`)
- Syntax highlighting with customizable colors
- SQLite-backed persistent history with import/export support

## Installation

Pre-built binaries are available from [GitHub Releases](https://github.com/eitsupi/arf/releases). You can install them using one of the following methods:

### Shell Installer (Linux/macOS)

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/eitsupi/arf/releases/latest/download/arf-console-installer.sh | sh
```

### winget (Windows)

```sh
winget install --id eitsupi.arf
```

### Manual Download

Download the archive for your platform from [GitHub Releases](https://github.com/eitsupi/arf/releases) and extract the binary to a directory in your `PATH`.

### Build from Source

```sh
cargo install --git https://github.com/eitsupi/arf.git
```

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

### Meta Commands

| Command | Description |
|---------|-------------|
| `:help`, `:h` | Open interactive help browser |
| `:info`, `:session` | Show session information (version, config path, etc.) |
| `:shell` | Enter shell mode |
| `:r` | Return to R mode |
| `:reprex` | Toggle reprex mode |
| `:autoformat` | Toggle auto-formatting (requires Air) |
| `:switch <version>` | Restart with different R version (requires rig) |
| `:restart` | Restart R session |
| `:history clear` | Clear command history |
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
auto_match = true       # Defaults to false on Windows (see Known Issues)
auto_suggestions = true # fish/nushell style history suggestions

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

## Experimental Features

Features in this section are under development and may change or be removed in future versions. Configure them under the `[experimental]` section.

### Spinner

Displays an animated spinner at the start of the line while R is evaluating code, providing visual feedback that the system is busy. **Disabled by default.**

To enable, set the `frames` option:

```toml
[experimental.prompt_spinner]
frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"  # Braille dots
color = "Cyan"              # Spinner color (default: Cyan)
```

**Configuration options:**

| Option | Default | Description |
|--------|---------|-------------|
| `frames` | `""` (disabled) | Animation frames (each character is one frame). |
| `color` | `"Cyan"` | Spinner color. Accepts standard ANSI color names: `Black`, `Red`, `Green`, `Yellow`, `Blue`, `Magenta`, `Cyan`, `White`, and their `Light` variants (e.g., `LightBlue`). |

**Frame style examples:**

```toml
# Braille dots (recommended)
frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"

# ASCII spinner (works in all terminals)
frames = "|/-\\"

# Block spinner
frames = "▖▘▝▗"
```

### Auto-completion while typing

Show the completion menu automatically after typing a minimum number of characters, without requiring Tab.

```toml
[experimental]
completion_min_chars = 3  # Show completions after 3 characters
```

When not set, completion requires pressing Tab (the default behavior). This is similar to radian's `complete_while_typing` feature.

### History forget

Automatically remove commands that produced errors from history. Similar to fish's [sponge](https://github.com/meaningful-ooo/sponge) plugin.

```toml
[experimental.history_forget]
enabled = true
delay = 2          # Keep last N failed commands for quick retry
on_exit_only = false  # Purge on each prompt (false) or only on exit (true)
```

**Configuration options:**

| Option | Default | Description |
|--------|---------|-------------|
| `enabled` | `false` | Enable automatic removal of failed commands. |
| `delay` | `2` | Number of recent failed commands to keep accessible for retry. Older failed commands are purged. |
| `on_exit_only` | `false` | If `true`, only purge when session ends. If `false`, purge on each prompt. |

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

**Restore from backup:**

```sh
# Restore history from a unified export file
arf history import --from arf --file ~/arf_backup.db
```

## Known Issues

### Error detection uses `options(error = ...)`

arf uses R's `options(error = ...)` to detect errors from packages like dplyr/rlang that output error messages to stdout instead of stderr. This is necessary for accurate error tracking in command history and the status indicator.

**Limitations**:
- If you set a custom error handler via `options(error = ...)`, arf will chain to your handler, but arf's handler takes precedence. Your handler will still be called after arf records the error.
- There is a slight performance overhead (~microseconds) on each prompt due to R API calls for checking and resetting error state. This is negligible in practice but may be noticeable in benchmarks.

### Auto-matching brackets disabled by default on Windows

> [!NOTE]
> On Windows, `auto_match` defaults to `false` because Windows Console and Windows Terminal do not support [bracketed paste mode](https://en.wikipedia.org/wiki/Bracketed-paste). Without bracketed paste, pasting code containing brackets or quotes triggers auto-matching, which corrupts the pasted text. To enable auto-matching on Windows, add `auto_match = true` to your `[editor]` configuration—but be aware that pasting may not work correctly.

### Windows Terminal flickering in TUI pagers

The help browser and other TUI pagers may flicker when scrolling in **Windows Terminal 1.23 and earlier**. This is because Windows Terminal stable versions do not support [Synchronized Output (DEC mode 2026)](https://github.com/microsoft/terminal/issues/8331), which prevents screen tearing during rapid updates.

**Workaround**: Install [Windows Terminal Preview](https://aka.ms/terminal-preview) (1.24+), which includes Synchronized Output support.

## Related Projects

- [radian](https://github.com/randy3k/radian) — A 21st century R console written in Python. arf draws inspiration from radian's design philosophy.

- [sircon](https://github.com/lrberge/sircon) — Simple R Console. A Windows-only R console with powerful autocomplete and a macro language for custom shortcuts. Some of sircon's advanced features are future goals for arf.

## Acknowledgements

arf is built upon the broad Rust ecosystem and the remarkable efforts of those who have created open-source tools for R. In particular, we would like to highlight the following projects:

- **[ark](https://github.com/posit-dev/ark)** — The `arf-libr` and `arf-harp` crates are derived from ark's `libr` and `harp` crates, which provide the foundation for embedding R in Rust applications. Windows initialization follows ark's pattern. [tree-sitter-r](https://github.com/r-lib/tree-sitter-r) powers syntax highlighting and code analysis.

- **[radian](https://github.com/randy3k/radian)** — arf's design and many features are inspired by radian, including shell mode, stderr formatting, and tab completion patterns. The feedback and discussions in radian's GitHub issues over the years have also been invaluable.

- **[felp](https://github.com/atusy/felp)** — The interactive fuzzy help browser concept was inspired by felp's `fuzzyhelp()` function.

- **[reedline](https://github.com/nushell/reedline)** — The line editor library from the Nushell project that powers arf's interactive editing.

## License

MIT
