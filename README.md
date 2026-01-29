<div align="center">

# 🐶 arf console

*Alternative R Frontend*

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

- **Fuzzy History Search** — fzf-style history search with `Ctrl+R`. Type fragments to find past commands quickly.

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
- SQLite-backed persistent history

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

[editor]
mode = "emacs"          # "emacs" or "vi"
auto_match = true
autosuggestion = true   # fish/nushell style history suggestions

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

[reprex]
enabled = false
comment = "#> "
autoformat = false      # Requires Air CLI

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

## Known Issues

### Error detection uses `options(error = ...)`

arf uses R's `options(error = ...)` to detect errors from packages like dplyr/rlang that output error messages to stdout instead of stderr. This is necessary for accurate error tracking in command history and the status indicator.

**Limitations**:
- If you set a custom error handler via `options(error = ...)`, arf will chain to your handler, but arf's handler takes precedence. Your handler will still be called after arf records the error.
- There is a slight performance overhead (~microseconds) on each prompt due to R API calls for checking and resetting error state. This is negligible in practice but may be noticeable in benchmarks.

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
