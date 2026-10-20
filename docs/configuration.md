# Configuration

arf uses a TOML configuration file following the XDG Base Directory specification.

> [!WARNING]
> The configuration file format is not yet stable and may change in future versions.

## Configuration File Location

The configuration file is located at:

- **Linux**: `~/.config/arf/arf.toml`
- **macOS**: `~/Library/Application Support/arf/arf.toml`
- **Windows**: `C:\Users\<user>\AppData\Roaming\arf\arf.toml`

Configuration selection follows `--config` > `ARF_CONFIG` > the platform
default. For example, `ARF_CONFIG=/path/to/arf.toml arf` selects one file for
commands that load configuration; files are not merged. `--config` remains
command-specific and follows that command's option syntax. When selected,
`ARF_CONFIG` must be non-empty, though explicit `--config` takes precedence
even over an empty value. `completions` and IPC clients ignore `ARF_CONFIG`,
and `config init` always writes to the platform default location.
`:info` shows the selected configuration path and source.

For example, the main command accepts a custom config file with `--config`:

```bash
arf --config /path/to/arf.toml
```

## Generating a Default Config

Use the built-in command to generate a default configuration file:

```bash
arf config init
```

To overwrite an existing config:

```bash
arf config init --force
```

## Default Configuration

If no configuration file exists, arf uses these defaults:

```toml
#:schema https://raw.githubusercontent.com/eitsupi/arf/main/artifacts/arf.schema.json

[startup]
r_source = "auto"       # How to locate R: "auto", "rig", or { path = "..." }
show_banner = true      # Show startup banner

reprex = "off"          # "off", "on", or "format"

[ipc.eval]
# Exact direct function/operator targets permitted by `arf ipc eval`; empty by default.
# Examples: ["mean", "stats::median", "+"]
allowed_functions = []

[editor]
mode = "emacs"          # Editing mode: "emacs" or "vi"
auto_match = true       # Auto-close brackets and quotes
highlight_matching_bracket = false  # Highlight matching bracket pair
auto_suggestions = "all" # History suggestions: "none", "all", or "cwd"

# Keyboard shortcuts (crokey format)
[editor.key_map]
"alt-hyphen" = " <- "      # Alt+- inserts assignment operator
"alt-p" = " |> "           # Alt+P inserts pipe operator (P = Pipe)

[prompt]
format = "{status}R {version}> "   # Main prompt (includes status indicator)
continuation = "+  "       # Continuation prompt for multiline input
shell_format = "[{shell}] $ "  # Shell mode prompt
mode_indicator = "prefix"  # Position of mode indicator: "prefix", "suffix", or "none"

[prompt.status]
override_prompt_color = false  # Also change entire prompt color based on status

[prompt.status.symbol]
success = ""               # Status symbol on success (empty = hidden)
error = "✗ "               # Status symbol on error

[prompt.vi.symbol]
insert = "[I] "            # Vi insert mode indicator
normal = "[N] "            # Vi normal mode indicator
non_vi = ""                # Non-vi modes (Emacs, etc.)

[prompt.indicators]
reprex = "[reprex] "       # Indicator text for reprex mode
reprex_format = "[format] " # Indicator text for formatted reprex mode

[completion]
enabled = true             # Enable tab completion
timeout_ms = 50            # Completion timeout in milliseconds
debounce_ms = 100          # Debounce delay for completion
max_height = 10            # Maximum height of completion menu
auto_paren_limit = 50      # Max packages to check for function paren insertion

[history]
menu_max_height = 15       # Maximum height of history search menu (Ctrl+R)
mode = "persistent"        # "persistent" or session-only "volatile"
# mode = { dir = "/custom/path" }  # Custom directory when mode is persistent

[r]
auto_width = true          # Sync R's options(width) with terminal size

[reprex]
comment = "#> "            # Comment prefix for reprex output
formatter = "auto"         # "auto", "air" (>= 0.9.0), or "arity" (>= 0.18.0)

# Syntax highlighting colors
[colors.r]
comment = "DarkGray"
string = "Green"
number = "LightMagenta"
keyword = "LightBlue"
constant = "LightCyan"
operator = "Yellow"
punctuation = "Default"
identifier = "Default"
matching_bracket = "LightYellow"  # Background color for matching bracket highlight

[colors.meta]
command = "Magenta"

[colors.prompt]
main = "LightGreen"
continuation = "LightGreen"
shell = "LightRed"
indicator = "Yellow"

[colors.prompt.status]
success = "LightGreen"     # Color for success (symbol and/or prompt)
error = "LightRed"         # Color for error (symbol and/or prompt)

[colors.prompt.vi]
insert = "LightGreen"     # Color for vi insert mode indicator
normal = "LightYellow"    # Color for vi normal mode indicator
non_vi = "Default"         # Color for non-vi modes (Emacs, etc.)

[experimental]
shell_semicolon_shortcut = false  # `;` at empty prompt switches to shell mode

[experimental.r_help]
viewer = "r"               # Default; opt in to the native pager with "auto"

[experimental.shell_abbreviations]
# Fish-style abbreviations for shell mode (expanded on Space/Enter)
# "gc" = "git commit"

[experimental.history_forget]
enabled = false            # Auto-remove failed commands from history
delay = 2                  # Keep last N failed commands for retry
on_exit_only = false       # Purge on each prompt (false) or only on exit (true)

[experimental.r_completion]
fuzzy = false              # Fuzzy matching for pkg::func and library() completions
package_functions = ["library", "require"]  # Functions that trigger package name completion

[experimental.r_completion.static.formals]
mode = "off"              # "off" or "prefer-static" for pkg::foo( argument completion

[experimental.r_completion.static.formals.exclusions]
packages = []              # Package names that always use R completion
functions = []             # Public names such as "pkg::function" that use R completion

[experimental.prompt_spinner]
frames = ""                # Animation frames (empty = disabled)
color = "Cyan"             # Spinner color

[experimental.prompt_duration]
format = "{value} "        # Duration display format ({value} = time string)
threshold_ms = 2000        # Show duration only for commands slower than this (ms)

```

## Bracket Highlighting

arf can highlight matching bracket pairs (`()`, `[]`, `{}`) when the cursor is on or immediately after a bracket. Both brackets are highlighted with a background color while preserving the syntax foreground color. The matching is syntax-aware via tree-sitter — brackets inside strings and comments are correctly ignored.

```toml
[editor]
highlight_matching_bracket = true

[colors.r]
matching_bracket = "LightYellow"  # Background color for both brackets
```

This feature is disabled by default. Set `matching_bracket` to `"Default"` to disable the background color while keeping bracket detection active.

## Auto Width

When `auto_width` is enabled (default), arf automatically syncs R's `options(width)` with the terminal width at startup and on resize. This ensures output from functions like `str()`, `print()`, and tibble printing uses the full available terminal width instead of R's default of 80 columns.

```toml
[r]
auto_width = true  # default
```

Set to `false` if you prefer to manage `options(width)` manually (e.g., via `.Rprofile`).

## Experimental R Help Viewer

R's own help commands use R's viewer by default (`viewer = "r"`). To open help results (for example, entering `?mean`) in arf's native pager in the interactive REPL, opt in under `[experimental.r_help]`. This experimental setting may change or be removed in future versions:

```toml
[experimental.r_help]
viewer = "auto"  # opt in to the native pager when safe to install
# viewer = "r"   # default; leave help display to R
```

With `viewer = "auto"`, arf installs a small `print.help_files_with_topic` wrapper only when the currently registered S3 method is identical to the standard method from `utils`. If another frontend has registered its own method, arf leaves the S3 registry unchanged. There is no force-install mode. The wrapper keeps the standard R method as its fallback: unsupported types such as HTML or PDF, missing topics, and requests that cannot be prepared continue through R's normal printer. If R resolves a request to multiple help pages, arf presents a selector and requires an explicit choice; cancelling the selector displays no page.

The setting is read when the interactive REPL starts; restart arf to change it. This integration is not installed for headless sessions, R scripts, or standalone mode. Assigning a help object alone does not display it; the pager is invoked when R prints the object.

The `:help` browser is available independently of this setting.

## Auto Suggestions

arf supports fish/nushell-style autosuggestions that appear as you type. These grayed-out suggestions can be accepted with the right arrow key.

### Configuration

```toml
[editor]
auto_suggestions = "all"  # "none", "all", or "cwd"
```

| Value | Description |
|-------|-------------|
| `"none"` | Disable suggestions |
| `"all"` | Show suggestions from all history (default) |
| `"cwd"` | Show suggestions only from current directory history |

For backward compatibility, boolean values are also accepted:
- `true` → `"all"`
- `false` → `"none"`

### CWD Mode

The `"cwd"` mode filters suggestions to show only history entries that were recorded in the current working directory. If no matches are found, it falls back to all history.

> [!NOTE]
> The `"cwd"` setting only affects R mode suggestions. Shell mode (`#!` prefix) always searches all history regardless of this setting.

## Shell Mode Completion

In shell mode, tab completion uses `ShellCompleter`.

### Configuration

```toml
[experimental.shell_completion]
command_names = false  # Suggest executable names from PATH at command position
```

`command_names` is disabled by default. When enabled, executable names from `PATH` are suggested only when the cursor is at command position (for example, the first token in a command segment).

### Behavior

- Meta commands (starting with `:`) are delegated to `MetaCommandCompleter`
- File and directory paths are completed at any token position
- Command segments are split by separators like `|` and `;`
- Paths containing spaces are wrapped in double quotes

### Known Limitations

- Quote-aware tokenization is not implemented. Pressing Tab again inside an already-quoted path may produce incorrect span positions.
- Quoting is optimized for common paths and does not fully escape all shell metacharacters. Paths containing uncommon characters (for example `$`, backticks, or embedded quotes) may require manual editing before execution.

## Keyboard Shortcuts

arf supports configurable keyboard shortcuts using the [crokey](https://github.com/Canop/crokey) format.

### Default Shortcuts

| Shortcut | Inserts | Config Key |
|----------|---------|------------|
| `Alt+-` | ` <- ` | `"alt-hyphen"` |
| `Alt+P` | ` \|> ` | `"alt-p"` |

> [!NOTE]
> arf uses `Alt+P` instead of the RStudio-style `Ctrl+Shift+M` because `Ctrl+Shift+M` conflicts with VS Code and Zed's diagnostics panels when running in their integrated terminals. See [Customizing for RStudio compatibility](#customizing-for-rstudio-compatibility) below.

### Key Format

Keys are specified in crokey format:

- Modifiers: `ctrl`, `alt`, `shift`
- Special keys: `hyphen`, `space`, `tab`, `enter`, `backspace`, `delete`, etc.
- Regular keys: `a`-`z`, `0`-`9`, punctuation

### Examples

```toml
[editor.key_map]
# Assignment operator: Alt+- → " <- "
"alt-hyphen" = " <- "

# Native pipe: Alt+P → " |> " (default)
"alt-p" = " |> "

# Magrittr pipe: Alt+M → " %>% "
"alt-m" = " %>% "

# Equality check: Alt+= → " == "
"alt-=" = " == "

# Right arrow: Ctrl+. → " -> "
"ctrl-." = " -> "
```

### Customizing for RStudio Compatibility

If you prefer RStudio-style shortcuts and are using a standalone terminal (not VS Code or Zed integrated terminal), you can use `Ctrl+Shift+M` for the pipe operator:

```toml
[editor.key_map]
"alt-hyphen" = " <- "
"ctrl-shift-m" = " |> "
```

> [!WARNING]
> `Ctrl+Shift+M` opens the Problems/Diagnostics panel in VS Code and Zed, so this shortcut won't reach arf when running in their integrated terminals.

### Disabling Default Shortcuts

To disable all shortcuts, set an empty table:

```toml
[editor.key_map]
```

## Color Configuration

arf supports configurable syntax highlighting colors for R code and meta commands.

### Available Colors

**Named Colors** (case-sensitive):
- Basic: `Black`, `Red`, `Green`, `Yellow`, `Blue`, `Purple`, `Magenta`, `Cyan`, `White`
- Light: `LightRed`, `LightGreen`, `LightYellow`, `LightBlue`, `LightPurple`, `LightMagenta`, `LightCyan`, `LightGray`
- Dark: `DarkGray`
- Special: `Default` (terminal default color)

**256-Color Palette**:
```toml
keyword = { Fixed = 99 }    # Color index 0-255
```

**True Color (RGB)**:
```toml
string = { Rgb = [0, 255, 128] }    # RGB values 0-255
```

### Token Types

| Token | Description | Default |
|-------|-------------|---------|
| `comment` | Lines starting with # | DarkGray |
| `string` | String literals | Green |
| `number` | Numeric literals | LightMagenta |
| `keyword` | if, else, for, while, function, etc. | LightBlue |
| `constant` | TRUE, FALSE, NULL, NA, Inf, NaN | LightCyan |
| `operator` | +, -, <-, \|>, etc. | Yellow |
| `punctuation` | Brackets, commas, semicolons | Default |
| `identifier` | Variable and function names | Default |
| `matching_bracket` | Background color for matching bracket highlight | LightYellow |

### Prompt Colors

| Setting | Description | Default |
|---------|-------------|---------|
| `main` | Main R prompt color | LightGreen |
| `continuation` | Continuation prompt color | LightGreen |
| `shell` | Shell mode prompt color | LightRed |
| `indicator` | Mode indicator text color ([reprex], [format], #!) | Yellow |
| `status.success` | Color for success (symbol and/or prompt when override_prompt_color is true) | LightGreen |
| `status.error` | Color for error (symbol and/or prompt when override_prompt_color is true) | LightRed |
| `duration` | Color for command duration indicator | Yellow |
| `vi.insert` | Color for vi insert mode indicator | Default |
| `vi.normal` | Color for vi normal mode indicator | Default |
| `vi.non_vi` | Color for non-vi modes (Emacs, etc.) | Default |

## Prompt Placeholders

The `prompt.format`, `prompt.continuation`, and `prompt.shell_format` fields support placeholder expansion:

| Placeholder | Description | Example |
|-------------|-------------|---------|
| `{version}` | R version number | `4.4.0` |
| `{cwd}` | Current working directory (full path) | `/home/user/project` |
| `{cwd_short}` | Current working directory (basename only) | `project` |
| `{shell}` | Shell name from $SHELL (Unix) or "cmd" (Windows) | `bash`, `zsh`, `cmd` |
| `{status}` | Command status indicator (see below) | `✗ ` on error |
| `{duration}` | Command execution time (see [Command Duration](#command-duration-indicator)) | `5s `, `1m30s ` |

### Prompt Examples

```toml
[prompt]
# Show R version in prompt with status indicator (default)
format = "{status}R {version}> "
# Result: "R 4.4.0> " on success, "✗ R 4.4.0> " on error

# Show short directory name
format = "[{cwd_short}] r> "
# Result: "[project] r> "

# Add a blank line before the prompt
# (double-quoted strings interpret \n as a newline)
format = "\n{status}R {version}> "

# Custom shell mode prompt
shell_format = "{shell}:{cwd_short}$ "
# Result: "bash:project$ "
```

## Command Status Indicator

arf can show a visual indicator when the previous command failed. This is similar to fish shell's default behavior.

### Configuration

The `prompt.status.symbol` table configures which symbols are shown via the `{status}` placeholder:

```toml
[prompt]
format = "{status}R {version}> "

[prompt.status]
symbol = { error = "✗ " }      # Show "✗ " on error, nothing on success
override_prompt_color = false  # Also change entire prompt color

[colors.prompt.status]
success = "LightGreen"   # Color for success (symbol and/or prompt)
error = "LightRed"       # Color for error (symbol and/or prompt)
```

### Examples

```toml
# Default: show colored symbol on error only
[prompt.status]
symbol = { error = "✗ " }

# Show checkmark on success, X on error
[prompt.status]
symbol = { success = "✓ ", error = "✗ " }

# No status symbols (disable)
[prompt.status]
symbol = {}

# Change entire prompt color on error (no symbol)
[prompt.status]
override_prompt_color = true

# Symbol + prompt color change
[prompt.status]
symbol = { error = "✗ " }
override_prompt_color = true
```

## Command Duration Indicator

arf can show how long the previous command took to execute via the `{duration}` prompt placeholder. This is an experimental feature.

The time format follows starship's convention: `5s`, `1m30s`, `2h48m30s` (no spaces between units, leading zero units skipped). For sub-second durations, milliseconds are shown (e.g., `800ms`).

> [!NOTE]
> `{duration}` is not included in the default prompt format. To use it, add `{duration}` to your `prompt.format` setting.

### Configuration

```toml
[prompt]
format = "{duration}{status}R {version}> "

[experimental.prompt_duration]
format = "{value} "   # How to display the duration ({value} = time string)
threshold_ms = 2000   # Only show for commands that take longer than 2s (default)

[colors.prompt]
duration = "Yellow"   # Color for duration text (default)
```

### How It Works

- The `format` string uses `{value}` as a sub-placeholder for the time string (e.g., "5s"). If `{value}` is omitted, only the static text in the format string is shown
- When the previous command exceeded `threshold_ms`, `{value}` in the format string is replaced with the time string, and the result replaces `{duration}` in the prompt
- When the command was fast (below threshold) or no command has been run yet, `{duration}` expands to an empty string
- The entire format string is conditional — static text in the format (like "took ") only appears when the duration is shown
- This means you can safely place `{duration}` in your prompt — it will only appear when relevant

### Examples

```toml
# Simple (default format): "5s R 4.4.0> " after slow command
[prompt]
format = "{duration}{status}R {version}> "

# starship-like: "took 5s R 4.4.0> "
[prompt]
format = "{duration}{status}R {version}> "
[experimental.prompt_duration]
format = "took {value} "

# Bracketed: "(5s) R 4.4.0> "
[prompt]
format = "{duration}{status}R {version}> "
[experimental.prompt_duration]
format = "({value}) "

# Lower threshold to 500ms (sub-second shows milliseconds like "800ms")
[experimental.prompt_duration]
threshold_ms = 500

# Custom color
[colors.prompt]
duration = "DarkGray"
```

## Vi Mode Indicator

arf can show a visual indicator for the current vi editing mode. This is useful when using vi keybindings to know whether you're in insert or normal mode.

The vi mode indicator is displayed at the end of the prompt (after the main prompt text), following the same approach as nushell.

### Default Behavior

By default, vi mode shows `[I]` and `[N]` indicators with colors:
- Insert mode: `[I] ` (LightGreen) → prompt appears as `R 4.4.0> [I] `
- Normal mode: `[N] ` (LightYellow) → prompt appears as `R 4.4.0> [N] `

Non-vi modes (Emacs) show no indicator by default.

### Symbol Configuration

| Field | Description | Default |
|-------|-------------|---------|
| `insert` | Symbol shown in vi insert mode | `"[I] "` |
| `normal` | Symbol shown in vi normal mode | `"[N] "` |
| `non_vi` | Symbol shown in non-vi modes (Emacs) | `""` (empty) |

### Color Configuration

| Field | Description | Default |
|-------|-------------|---------|
| `insert` | Color for vi insert mode indicator | LightGreen |
| `normal` | Color for vi normal mode indicator | LightYellow |
| `non_vi` | Color for non-vi modes (Emacs) | Default |

### Examples

```toml
# Nushell-style: mode-aware prompt suffix
[prompt]
format = "R {version} "   # No trailing ">" - the vi indicator provides it
[prompt.vi]
symbol = { insert = "> ", normal = ": ", non_vi = "> " }

# Unicode indicators
[prompt.vi]
symbol = { insert = "● ", normal = "○ " }

# Custom colors
[colors.prompt.vi]
insert = "Green"
normal = "Yellow"

# Disable vi mode indicator (set symbols to empty strings)
[prompt.vi]
symbol = { insert = "", normal = "" }
```

> [!NOTE]
> To disable the vi mode indicator entirely, set the symbols to empty strings as shown above.

## Reprex Modes and Formatting

arf supports three reprex modes: `off`, `on`, and `format`. The `format` mode
formats R code before reprex evaluation using the configured formatter backend.
The default `formatter = "auto"` selector prefers Air and then Arity when both
are available. Explicit `air` and `arity` selections are strict and never
fall back to the other backend.
The supported backends are [Air](https://github.com/posit-dev/air) 0.9.0 or later
and [Arity](https://github.com/jolars/arity) 0.18.0 or later. Air receives code
over stdin with the virtual path `arf-reprex.R` and `--force`, preserving project
configuration discovery from the current working directory and avoiding
temporary files. Arity receives code over stdin with `arity format -` and also
uses the current working directory for configuration discovery. If formatting
fails, arf reports the backend's stderr and does not evaluate the unformatted
code.

### Configuration

```toml
[startup]
reprex = "format"  # "off", "on", or "format"
```

### CLI Option

```bash
# Enable reprex mode with formatting
arf --reprex=format
```

### Runtime Selection

Select the mode during a session using the explicit meta command:

```
:reprex on
:reprex off
:reprex format  # Requires the configured formatter (auto prefers Air, then Arity)
```

## R Source Configuration

arf supports multiple ways to locate the R installation.

### Configuration

```toml
[startup]
# Option 1: Auto-detect (default)
# Uses rig if available, otherwise finds R from PATH
r_source = "auto"

# Option 2: Explicitly use rig
# Requires rig to be installed
r_source = "rig"

# Option 3: Explicit path to R_HOME
r_source = { path = "/opt/R/4.5.2" }
```

### Version Specifications

arf uses the same installed-version matching for `--with-r-version`, `:switch`, and R source overrides. `--with-r-version` and `:switch` additionally accept rig's own selectors, which are tried first — see [rig Integration](#rig-integration). An R version is a plain `major.minor.patch` number: R does not publish prereleases or build metadata, and rig reports installed R versions in that form. For installed-version matching, arf accepts two forms:

- Exact or partial version numbers: `4`, `4.4`, or `4.4.2`. The number of components written determines the precision: `4.4` matches the `4.4.x` series, while `4.4.2` matches only `4.4.2`.
- Ranges using comparison operators such as `^4.4`, `~4.4`, `>=4.3`, `<5.0`, and `*`. Operators can be combined, for example `>=4.3, <5.0`.

The range operators use the syntax popularised by Cargo and npm. This is only range notation for selecting R versions; R version numbers are not SemVer. Because an R version has only three components, numeric specifications with four or more components, such as `4.4.1.0`, cannot match and are rejected. Likewise, prerelease identifiers and build metadata are rejected because installed R versions never carry them. If multiple installed versions match, arf selects the newest one.

### CLI Options

The `--r-home` flag specifies an explicit R_HOME path:

```bash
arf --r-home /opt/R/4.5.2
```

The `--with-r-version` flag temporarily overrides `r_source` and uses rig. It accepts one of the version specifications above or a selector from rig's own metadata:

```bash
arf --with-r-version 4.5
```

The `--r-provider` flag explicitly selects an active provider environment or synced rig project:

```bash
arf --r-provider rig-project
arf --r-provider pixi
```

These options are mutually exclusive.

To ask arf which R installation it would use without starting R, run `arf r resolve`. It accepts the same `--r-home`, `--with-r-version`, `--r-provider`, `--no-r-source-overrides`, and `--config` options as the startup path; see [`arf r resolve`](r-resolve.md) for the machine-facing JSON interface.

### rig Integration

When using rig via `r_source = "auto"` with rig installed or `r_source = "rig"`, arf uses rig's default version. The `--with-r-version` flag accepts an explicit specification: `--with-r-version default` selects rig's default, while another specification selects the matching installed version. You can change the default with:

```bash
rig default 4.5
```

Rig selectors are separate from version specifications:

| Selector | Description |
|--------------|-------------|
| `default` | Use rig's default R version |
| Rig alias (e.g. `release` or `devel`) | Use the version associated with that rig alias |
| Rig-assigned name (e.g. `custom-name`) | Use the installed version with that rig name |

`--with-r-version` and `:switch` try these selectors before interpreting the value as a version specification, in the order listed above. R source overrides never consult them and always interpret the value as a version specification. This matters when a rig alias or name looks like a version number: if an installation is named `4.4`, then `--with-r-version 4.4` selects that installation by name, while `4.4` in an override file selects the newest installed `4.4.x` release, which may be a different installation.

### Switching and Restarting

`:switch <version>` and `:restart` both restart arf, but they differ in how they treat the environment:

- `:switch <version>` uses arf's original pre-initialization startup snapshot. It restores the startup values of `R_LIBS_USER`, `R_LIBS_SITE`, `R_LIBS`, `R_DOC_DIR`, `R_SHARE_DIR`, `R_INCLUDE_DIR`, and `R_SYSTEM_ABI`, if they were present when arf first started; if one was absent, arf removes it before restarting so the new R can compute it. The snapshot remains available across restarts. `R_HOME` is always removed because arf sets it from the resolved R version, so an inherited value is not meaningful. `LD_LIBRARY_PATH` is also always removed to preserve current behavior, even when it had a user-set value at startup; restoring that value is a future improvement. These rules prevent values introduced by R or the session from leaking into the new version while preserving the other startup values. If any variables are affected, arf prints their names but never their values, for example:

  ```text
  # [arf] Environment variables for the R version switch: restored: R_LIBS_USER; removed: R_HOME, LD_LIBRARY_PATH
  ```

  For values that should persist across R version switches, prefer `~/.Renviron` over shell environment variables. R reads that file afresh for each version, so its `${VAR-'default'}` handling can calculate paths for the selected installation instead of carrying one exact shell value across versions.

- `:restart` relaunches the same version without touching environment variables, so user-set values and anything set during the session with `Sys.setenv()` carry over.

## History Configuration

History schema, export, and import commands also honor `ARF_CONFIG`; for
example, `arf --config /path/to/arf.toml history schema` selects a file for the
schema command.

### Configuration

```toml
[history]
menu_max_height = 15   # Maximum height of Ctrl+R menu
mode = "persistent"   # "persistent" loads/saves SQLite; "volatile" is session-only
# For a custom persistent directory, use instead:
# mode = { dir = "/custom/path" }
```

### Environment Variable

The `ARF_HISTORY_DIR` environment variable can be used to override the history directory. This is useful for devcontainer Features that persist history via Docker volumes.

```bash
export ARF_HISTORY_DIR=/dc/arf-history
```

### Priority Order

The history directory is resolved with the following priority (highest first):

1. CLI `--history-dir`
2. `ARF_HISTORY_DIR` environment variable
3. TOML `[history] mode = { dir = "..." }`
4. XDG default

### CLI Options

```bash
arf --no-history              # Use volatile session-only history (no disk load/save)
arf --history-dir /path/to   # Custom history directory
```

History files are stored as SQLite databases:
- R history: `{dir}/r.db`
- Shell history: `{dir}/shell.db`

Default location:
- **Linux**: `$XDG_STATE_HOME/arf/history/` (`~/.local/state/arf/history/`). Earlier versions used `$XDG_DATA_HOME/arf/history/` (`~/.local/share/arf/history/`); if that directory contains `r.db` or `shell.db` and the new one does not, arf keeps using the old directory for both R and shell history and warns at startup. If both contain history, the new directory is used.
- **macOS**: `~/Library/Application Support/arf/history/`
- **Windows**: `C:\Users\<user>\AppData\Local\arf\history\`

### Exporting and Importing History

You can export your history to a backup file:

```bash
arf history export --file backup.db
```

To restore or transfer history to another machine:

```bash
arf history import --from arf --file backup.db
```

You can also import history from other sources:

```bash
# Import from radian
arf history import --from radian

# Import from standard R history file
arf history import --from r
```

> [!NOTE]
> Re-importing the same file is safe — duplicate entries are automatically skipped by matching command text and timestamp.

To copy history from the legacy default location (`~/.local/share/arf/history/` on Linux) to the current default location (`~/.local/state/arf/history/`), use `arf history migrate` (add `--dry-run` to preview). It skips duplicates, never modifies the legacy location, and ignores explicit history directories (`--history-dir`, `ARF_HISTORY_DIR`, `history.mode = { dir = ... }`). Close other arf sessions before running it. Migrated entries are appended after any existing history, so migrate before building up new history in the current location to keep older entries ordered first. It never deletes anything; once you have confirmed your history in the new location, you may delete the legacy directory yourself, but arf versions from before the move still read it.

## Experimental Features

Features in this section are under development and may change or be removed in future versions.

### Spinner

Displays an animated spinner at the start of the line while R is evaluating code. **Disabled by default** — set `frames` to enable.

```toml
[experimental.prompt_spinner]
frames = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"  # Braille dots
color = "Cyan"
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

### Fuzzy R Completion

Use fuzzy matching for R code completions. When enabled, typing `sf::geo` can match `sf::st_geometry` and `library(dpl` can match `dplyr`. **Disabled by default.**

```toml
[experimental.r_completion]
fuzzy = true
```

Both `::` (exported names) and `:::` (internal names) are supported. Package exports are cached per-package with a 5-minute TTL for performance.

The `package_functions` option controls which function calls trigger package-name fuzzy completion (defaults to `["library", "require"]`). Add custom functions as needed:

```toml
[experimental.r_completion]
fuzzy = true
package_functions = ["library", "require", "box::use"]
```

#### Static formal completion

When enabled, arf can inspect installed package metadata and stored code to
complete formal argument names in qualified calls such as `stats::lm(` without
asking R to evaluate the completion request. The static path is conservative:
unsupported, ambiguous, or unavailable metadata always falls back to R's
completion oracle. It is disabled by default while this experimental feature is
being evaluated.

```toml
[experimental.r_completion.static.formals]
mode = "prefer-static"

[experimental.r_completion.static.formals.exclusions]
packages = ["S7", "methods"]
functions = ["rlang::abort"]
```

`mode = "off"` retains the existing R-only behavior. `mode = "prefer-static"`
uses static formals only for a clear static hit and otherwise falls back to R.
Exclusions are case-sensitive exact matches; function entries use the public
`pkg::function` spelling and package entries take precedence. These exclusions
apply only to static formal completion.

If you used an earlier development build, rename
`[experimental.r_completion.static_formals]` to
`[experimental.r_completion.static.formals]` and move its `.exclusions` table
to the same new hierarchy. The old key is not an alias and no longer enables
static formal completion. The configuration file is still named `arf.toml`.

### History Forget

Automatically removes commands that produced errors from history. Similar to fish's [sponge](https://github.com/meaningful-ooo/sponge) plugin.

> [!NOTE]
> History forget only applies to commands typed interactively by the user in the REPL. It does not apply to headless mode or commands sent via IPC, because agent-sent commands are valuable as an execution log and should not be silently pruned just because they failed.

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

### Shell Semicolon Shortcut

Pressing `;` at an empty R prompt instantly switches to shell mode — no `:shell` or Enter required. Similar to [Julia REPL](https://docs.julialang.org/en/v1/stdlib/REPL/#man-shell-mode) shell mode behavior. **Disabled by default.**

```toml
[experimental]
shell_semicolon_shortcut = true
```

When the buffer is not empty, `;` inserts a semicolon as usual, so normal R expressions like `for (i in 1:10) { ... }` are unaffected.

### Shell Abbreviations

Fish-style abbreviations for the shell editor. When you type an abbreviation and press Space or Enter, it is automatically expanded to the full text. Only applies to shell mode — the R editor is unaffected.

**Disabled by default** (empty map).

```toml
[experimental.shell_abbreviations]
"gc" = "git commit"
"gp" = "git push"
"gs" = "git status"
```

Abbreviations are matched against the word immediately to the left of the cursor at the moment you press Space or Enter. The expansion replaces the abbreviation in place.

### R Source Overrides

Select R from project version files, a synced rig project, or an externally activated Pixi environment. This feature is fully opt-in: `r_source_overrides` defaults to an empty array. If it is unset or empty, arf falls back entirely to `startup.r_source`, exactly as before.

> [!IMPORTANT]
> A version read from a file is only ever matched against the R installations that rig knows about — arf takes the candidate list from `rig list --json`. **The `version-file`, `toml-key`, and `json-key` providers therefore require rig**, and can only select an R version that rig has already installed. Without rig, or when no installed version matches, arf warns and tries the next provider, including rig-project or Pixi. It falls back to `startup.r_source` only after all providers have been exhausted.

Override files are resolved as `<current directory>/<file>`. `file` must be a bare filename: it cannot be empty, `.`, `..`, contain subdirectories, or be an absolute path. arf does **not** walk up parent directories, even though the tools that write these files often do.

arf uses the `file` value as written and performs no case folding, so whether `.r-version` also matches a file named `.R-version` depends on the filesystem: case-insensitive on typical macOS and Windows setups, case-sensitive on typical Linux ones. Write the exact spelling your project uses to keep the config portable across platforms.

Entries are evaluated in array order, which is the priority order. The first entry that successfully resolves an R source is used; later entries are not evaluated once one succeeds.

**Configuration options:**

| Option | Default | Description |
|--------|---------|-------------|
| `r_source_overrides` | `[]` (disabled) | Ordered list of R source providers. |
| `provider = "version-file"` | — | Reads the first non-empty line as the version specification from the `file` field. |
| `provider = "toml-key"` | — | Reads a string version specification from the dot-separated TOML key in the `file` and `key` fields. |
| `provider = "json-key"` | — | Reads a string version specification from the dot-separated JSON key in the `file` and `key` fields. |
| `provider = "rig-project"` | — | Uses R from the synchronized rig project in the current directory. Requires `rproj.toml` and a successful `rig proj status --json` check. |
| `provider = "pixi"` | — | Uses R from an externally activated Pixi environment, without requiring rig. Has no additional fields. |

For example, [rv](https://a2-ai.github.io/rv-docs/) stores its R version in `rproject.toml`. This configuration reads the `project.r_version` string and tries to select the matching installed R version:

```toml
[experimental]
r_source_overrides = [
  { provider = "toml-key", file = "rproject.toml", key = "project.r_version" },
]
```

The other provider forms are:

```toml
{ provider = "version-file", file = ".r-version" }
{ provider = "rig-project" }
{ provider = "pixi" }
```

**File formats read by each provider:**

`version-file` reads the first non-empty line and trims its leading/trailing whitespace; the trimmed result is used as the version specification. Values longer than 256 bytes are rejected. The format follows the version-file convention popularised by other ecosystems' version managers — `.python-version`, `.ruby-version` and similar files — a single version on its own line, e.g. a `.r-version` containing:

```
4.4.1
```

Comments are not supported. The first non-empty line may be any supported form, including a range such as `>=4.3, <5.0`; later lines are ignored. Only spaces are accepted inside a specification, so a range must stay on one line.

`toml-key` parses `file` as TOML and follows `key` as a dot-separated path through its tables. For example, rv's `rproject.toml` might contain:

```toml
[project]
name = "my-analysis"
r_version = "4.4"
```

With `key = "project.r_version"`, arf looks up the `project` table and reads its `r_version` field. The value must be a TOML string; any other type is treated as an error.

`json-key` parses `file` as JSON and follows `key` as a dot-separated path through its objects. For example, an `renv.lock` file contains the recorded R version near its top level:

```json
{
  "R": {
    "Version": "4.4.1",
    "Repositories": []
  }
}
```

With `key = "R.Version"`, arf reads the string under `R.Version`. The value must be a JSON string; arrays, array indexes, and escaped literal dots are not supported. This provider is fully opt-in; arf does not detect `renv.lock` automatically. A recorded `R.Version` is the version captured at the time of the snapshot, so updating the lockfile can change which R arf selects. Use this configuration only when that behavior is desired (and, when appropriate, use `renv::settings$r.version("4.4.1")` to intentionally control the recorded value):

```toml
[experimental]
r_source_overrides = [
  { provider = "json-key", file = "renv.lock", key = "R.Version" },
]
```

**Version strings read by `version-file`, `toml-key`, and `json-key` use the version specifications above.** These providers accept exact or partial numbers and ranges, and select the newest installed version that matches. `devel` and `release` are recognised names, but named selectors are not supported by the R source override path.

**Who performs the matching:** arf runs rig only to check that it is there (`rig --version`) and to list what is installed (`rig list --json`). It then matches the specification against that list itself and asks the selected installation's R binary for its `R_HOME`. rig never sees the specification, so it is arf that decides what `4.4` means.

`--with-r-version`, `:switch` and `r_source_overrides` share the numeric and range matching described above. Only selectors from rig's own metadata differ: `default`, aliases such as `release`, and exact rig names work with `--with-r-version` and `:switch`, while named selectors are unsupported in an override file.

**Rig projects**

`rig-project` reads the project selected by `rig proj status --json` from the current directory's `rproj.toml`. It accepts only a project that rig reports as current and synchronized, with no status warnings; arf does not create or synchronize the environment. Run `rig proj sync` after changing project dependencies or when rig asks to rebuild the environment. `arf r resolve` reports the verified project wrapper and rig installation name; the installation name is not treated as an exact R version.

For example, add `{ provider = "rig-project" }` to `experimental.r_source_overrides`, or explicitly select it with `arf --r-provider rig-project` or `ARF_R_PROVIDER=rig-project`. Explicit selection is strict: if the current directory is not a synced rig project, arf reports an error instead of choosing another R. `startup.r_source = "rig"` remains the normal rig default selection.

During normal R startup, R reads the project `.Renviron` and rig's `rvenv` package activates the project library. `--no-environ` and `--vanilla` suppress reading the project `.Renviron` for that startup, though environment variables inherited by arf may still affect R. `:restart` resolves the project at the current working directory. It clears `R_DOC_DIR`, `R_SHARE_DIR`, and `R_INCLUDE_DIR` only when arf derived them from the previous R installation and they were absent from the original startup environment, allowing the selected installation to recompute them. Other environment variables remain inherited, so changing directories does not fully reset project activation state. `:switch VERSION` explicitly selects an installed rig R version; rig then decides during R startup whether that project's library can be used with the selected version.

The integration test has validated user-mode rig 0.11.0 project startup and loading compiled `jsonlite` from the project library on Linux x64, macOS ARM64, and Windows x64. Its macOS TUI test covers R 4.6.1, 4.6.0, and 4.5.3, including a same-root restart after switching; it does not assert that rig deactivates the project library after a minor-version mismatch. The macOS integration test also validates `:restart` after changing the working directory from project A (R 4.6.1) to synchronized project B (R 4.6.0), including reevaluation of the current project.

rig 0.11.0 has an upstream issue where switching across R minor versions can leave an incompatible project library active after rig warns ([rig issue #450](https://github.com/r-lib/rig/issues/450), fix merged in [rig PR #451](https://github.com/r-lib/rig/pull/451)). Until using a rig release containing the fix and running `rig proj sync`, avoid `:switch` across R major/minor versions. arf does not restrict these switches or synchronize projects automatically. The mismatch was observed with native R and arf on macOS and reproduced on Linux with a minimal rig-source test; Windows has not been independently reproduced.

**Active Pixi environments**

Enable the provider in the loaded `arf.toml`, then start arf through Pixi:

```toml
[experimental]
r_source_overrides = [{ provider = "pixi" }]
```

```sh
pixi run arf
pixi run arf r resolve
pixi run arf headless --json
```

Pixi selects and activates the environment. arf does not invoke the Pixi CLI, create or update environments, run activation scripts, install packages, or change lockfiles. Named and detached environments are accepted through the same activation identity; arf does not assume a `.pixi/envs/default` layout. A valid active environment takes precedence over a different workspace in the current directory.

arf checks Pixi's activation variables and the read-only `conda-meta/pixi` marker. `PIXI_WORKSPACE_MANIFEST` takes precedence over legacy `PIXI_PROJECT_MANIFEST`, including when the newer variable is empty or invalid. `PIXI_IN_SHELL` must be present, and the manifest, environment name and `CONDA_PREFIX` must be non-empty and agree with the marker. `CONDA_PREFIX` alone does not identify Pixi. arf does not check environment freshness or repair an invalid marker.

The first native R executable on PATH, the `R_HOME` it reports, and its shared library must belong to the canonical Pixi prefix. If the first executable is outside the prefix, arf does not search for another one later on PATH. The RHOME probe removes inherited `R_HOME` in its child process and has a 10-second deadline, with stdout and stderr each limited to 64 KiB. A failed probe rejects that provider and allows the next provider to run. These per-probe limits also apply to automatic discovery and explicit installation-prefix resolution, including after a provider is rejected.

If activation is missing, arf only checks the exact current directory for a workspace in `pixi.toml` or `pyproject.toml` and suggests `pixi run arf` when one is found. It does not search parent directories. Package-only manifests, ordinary Python projects and malformed TOML do not trigger workspace guidance. With no activation signals or matching workspace, the provider is silently skipped.

Unavailable or invalid Pixi environments produce a warning and allow later providers, followed by `startup.r_source`, to run. No confirmation prompt is shown, including in headless mode. Once Pixi has been selected, library loading or R initialization failures are startup errors: arf does not silently start another R. `:restart` inherits and revalidates activation; `:switch` is unavailable for Pixi sessions. On Windows, only a selected Pixi source uses prefix-limited dependency DLL directories; ordinary R installations retain their existing DLL search policy.

REPL, script and plain headless startup warnings go to stderr once. JSON headless startup includes them in `warnings`; `arf r resolve` includes them in `diagnostics` without extra human-readable stderr. An unknown Pixi R version is reported as `null`, and resolve reports the verified PATH executable, which can live outside `R_HOME` on Windows.

The production provider has been validated on Linux x64, macOS ARM64 and Windows x64 using Pixi 0.81.0 with conda-forge R 4.5.3. The dedicated integration workflow covers `pixi run` in default and named detached environments, compiled `stats`, `jsonlite` and `curl`, eval/script/headless startup, and restart from a different workspace. Windows initialization converts canonical drive/UNC paths to the native spelling R accepts while preserving the validated target. Windows ARM64, `pixi exec`, global environments and standalone Pixi scripts are outside the initial verified scope.

When resolving file providers, a missing file is silently skipped. An unreadable or invalid value produces a warning and arf moves to the next entry.

When a provider's requested version is not installed, arf prints installation guidance and tries the next provider. Only after all providers fail does it fall back to `startup.r_source`; startup continues rather than aborting. If rig is unavailable or its version list cannot be read, the affected file provider is skipped and later providers are still evaluated. Numeric version selectors use `rig add`, while version ranges use non-command guidance because a range is not an executable `rig add` argument. The warnings are:

```text
Warning: rig is not installed, so the R source override cannot be resolved.
         Install rig from https://github.com/r-lib/rig or use "auto".
         Trying the next R source override.

Warning: R source override provider 'version-file' at .r-version requested R version "4.4", which is not installed.
         Install it with rig add 4.4, then restart arf.
         Trying the next R source override.

Warning: R source override provider 'version-file' at .r-version has no installed R version matching specification ">=4.3, <5.0".
         Install a matching R version with rig, then restart arf.
         Trying the next R source override.

Warning: All R source overrides failed.
         Using startup.r_source: PATH.
```

Use `--no-r-source-overrides` to disable evaluation of `r_source_overrides`. It does not disable explicit `--r-home`, `--with-r-version`, `--r-provider`, `ARF_R_HOME`, `ARF_R_VERSION`, or `ARF_R_PROVIDER` selection.

## R Source Precedence

The first two tiers are explicit CLI and environment selections. Tiers 3 and 4 are settings in the single loaded `arf.toml` (`r_source_overrides` and `startup.r_source`), selected by `--config`, `ARF_CONFIG`, or the platform default (see [Configuration File Location](configuration.md#configuration-file-location)). Project-local files such as `rproject.toml` and `.r-version` are inputs to override providers, not separate arf configuration files. Tiers 5 and 6 describe how arf discovers R after the selected configuration resolves to PATH mode.

For `headless` and `r resolve`, explicit R source flags belong to the subcommand: write `arf headless --r-provider rig-project` or `arf r resolve --r-provider rig-project`, not `arf --r-provider rig-project headless` or `arf --r-provider rig-project r resolve`. `ARF_R_HOME`, `ARF_R_VERSION`, and `ARF_R_PROVIDER` need no placement. A CLI selection takes precedence over all three environment variables. If no CLI selection is present and more than one environment variable is set, arf reports a conflict. An explicit rig-project provider requires a synced project in the current directory; an explicit Pixi provider requires an already active Pixi environment. Either request fails instead of selecting another R source.

| Tier | Source | Evaluation behavior |
|------|--------|---------------------|
| 1 | CLI `--r-home`, `--with-r-version`, or `--r-provider` | Returns immediately with the requested source; no lower tier is evaluated. These CLI flags conflict with each other and take precedence over R source environment variables. |
| 2 | `ARF_R_HOME`, `ARF_R_VERSION`, or `ARF_R_PROVIDER` | Used only when no CLI source is given. Setting more than one is an error. The interactive console, `headless` and `r resolve` read these variables for themselves. |
| 3 | `r_source_overrides` (setting in the loaded `arf.toml`) | Evaluates providers in order. A file provider may have no matching file or version, and an active-environment provider may be unavailable; those cases fall through to the next provider and then to tier 4. |
| 4 | `startup.r_source` (setting in the loaded `arf.toml`) | Resolves the configured source after the override providers have been exhausted; when it resolves here, source selection ends. |
| 5 | Inherited `R_HOME` | Not a selection tier. It is a discovery-layer input consulted only when tier 4 resolves to PATH mode. |
| 6 | `R RHOME` / built-in default paths | Final fallback search used to discover R when PATH-mode resolution needs it. |

Specifying `--r-home`, `--with-r-version`, or `--r-provider` (or any corresponding `ARF_R_*` variable) skips the `r_source_overrides` detection step entirely: project files are not read and no override warning is emitted. Inherited `R_HOME` likewise matters only as a discovery-layer input when `startup.r_source` falls into PATH mode.

> [!WARNING]
> With the default `startup.r_source = "auto"`, arf uses rig's default R directly when rig is available and its default can be resolved; otherwise it falls back to the R found on PATH. This does not guarantee that arf and an editor use the same installation: for example, a conda-provided R can appear before rig's shim on PATH, so the editor may discover it while arf uses rig's default.
>
> `r_source_overrides` and `ARF_R_HOME` / `ARF_R_VERSION` select R independently of PATH, so arf's R can silently diverge from the editor's. That breaks integrations assuming a shared installation, because installed packages and library paths are resolved against the editor's R.
>
> In an IDE-integrated workflow, prefer having the editor ask [`arf r resolve`](r-resolve.md) which R arf would use. Otherwise, consider leaving both disabled; if you enable them, keep the selection in sync with the editor, or have the editor launch arf with `--r-home` pointing at its own R.

## Other CLI Options

Command-line options take precedence over their corresponding config file settings:

| CLI Option | Config Setting |
|------------|----------------|
| `--no-banner` | `startup.show_banner` |
| `--reprex=<off\|on\|format>` | `startup.reprex` |
| `--no-history` | `history.mode = "volatile"` |
| `--history-dir` / `ARF_HISTORY_DIR` | `history.mode = { dir = "..." }` |

Example:
```bash
# Enable reprex mode with formatting
arf --reprex=format
```
