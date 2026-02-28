# Configuration

arf uses a TOML configuration file following the XDG Base Directory specification.

> [!WARNING]
> The configuration file format is not yet stable and may change in future versions.

## Configuration File Location

The configuration file is located at:

- **Linux**: `~/.config/arf/arf.toml`
- **macOS**: `~/Library/Application Support/arf/arf.toml`
- **Windows**: `C:\Users\<user>\AppData\Roaming\arf\arf.toml`

You can also specify a custom config file with the `--config` flag:

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

[startup.mode]
reprex = false          # Enable reprex mode
autoformat = false      # Enable auto-formatting (requires air)

[editor]
mode = "emacs"          # Editing mode: "emacs" or "vi"
auto_match = true       # Auto-close brackets and quotes
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
autoformat = "[format] "   # Indicator text for autoformat mode

[completion]
enabled = true             # Enable tab completion
timeout_ms = 50            # Completion timeout in milliseconds
debounce_ms = 100          # Debounce delay for completion
max_height = 10            # Maximum height of completion menu
auto_paren_limit = 50      # Max packages to check for function paren insertion

[history]
menu_max_height = 15       # Maximum height of history search menu (Ctrl+R)
disabled = false           # Disable history entirely
# dir = "/custom/path"     # Custom history directory (optional)

[mode.reprex]
comment = "#> "            # Comment prefix for reprex output

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

[experimental.history_forget]
enabled = false            # Auto-remove failed commands from history
delay = 2                  # Keep last N failed commands for retry
on_exit_only = false       # Purge on each prompt (false) or only on exit (true)

[experimental.completion_namespace]
fuzzy = false              # Fuzzy matching for pkg::func completions

[experimental.prompt_spinner]
frames = ""                # Animation frames (empty = disabled)
color = "Cyan"             # Spinner color

[experimental.prompt_duration]
format = "{value} "        # Duration display format ({value} = time string)
threshold_ms = 2000        # Show duration only for commands slower than this (ms)
```

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

## Auto-Formatting (Reprex Mode)

arf supports auto-formatting of R code using [air](https://github.com/posit-dev/air).

**Note:** Auto-formatting only applies in reprex mode.

### Configuration

```toml
[startup.mode]
reprex = true       # Enable reprex mode
autoformat = true   # Enable auto-formatting
```

### CLI Option

```bash
# Enable reprex mode with auto-formatting
arf --reprex --auto-format
```

### Runtime Toggle

Toggle during a session using meta commands:

```
:autoformat   # Toggle auto-formatting
:format       # Same as :autoformat
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

### CLI Options

The `--r-home` flag specifies an explicit R_HOME path:

```bash
arf --r-home /opt/R/4.5.2
```

The `--with-r-version` flag temporarily overrides `r_source` and uses rig:

```bash
arf --with-r-version 4.5
```

These options are mutually exclusive.

### rig Integration

When using rig (either via `r_source = "auto"` with rig installed, `r_source = "rig"`, or `--with-r-version`), arf uses rig's default version. You can change the default with:

```bash
rig default 4.5
```

The `--with-r-version` flag supports version resolution:

| Specification | Description |
|--------------|-------------|
| `default` | Use rig's default R version |
| `release` | Use the version aliased as "release" |
| `4.5` | Match the first version starting with "4.5" |
| `4.5.2` | Match exact version |

## History Configuration

### Configuration

```toml
[history]
menu_max_height = 15   # Maximum height of Ctrl+R menu
disabled = false       # Disable history
dir = "/custom/path"   # Custom history directory (optional)
```

### CLI Options

```bash
arf --no-history              # Disable history
arf --history-dir /path/to   # Custom history directory
```

History files are stored as SQLite databases:
- R history: `{dir}/r.db`
- Shell history: `{dir}/shell.db`

Default location (XDG data directory):
- **Linux**: `~/.local/share/arf/history/`
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

## CLI Options Override

Command-line options take precedence over config file settings:

| CLI Option | Config Setting |
|------------|----------------|
| `--r-home` | Overrides `startup.r_source` (explicit path) |
| `--with-r-version` | Overrides `startup.r_source` (uses rig) |
| `--no-banner` | `startup.show_banner` |
| `--reprex` | `startup.mode.reprex` |
| `--auto-format` | `startup.mode.autoformat` |
| `--no-history` | `history.disabled` |
| `--history-dir` | `history.dir` |

Example:
```bash
# Enable reprex mode with auto-formatting
arf --reprex --auto-format
```
