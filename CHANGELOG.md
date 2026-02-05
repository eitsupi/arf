# Changelog

## [Unreleased]

## [0.2.0-rc.3] - 2026-02-05

### Added

- Experimental history browser for interactive history management with search, filtering, copy, and delete support (#38)
  - Column headers, exit code column, and working directory column (#47)
  - Minimum terminal size warning for pager browsers (#50)
- Experimental `arf history import` subcommand for importing history from radian, R, or another arf database (#31)
- Experimental `arf history export` subcommand for backing up history to a unified SQLite file (#54)
  - Exports both R and shell history to a single file with customizable table names
  - Use with `arf history import --from arf` to restore or transfer history
- `editor.auto_suggestions` now supports `"cwd"` mode for directory-aware suggestions (#55)
  - When set to `"cwd"`, suggestions are filtered to history entries recorded in the current directory
  - Falls back to all history if no matches found
- Enhanced `:info` meta command with pager view, clipboard copy, and path masking (#29)
- Vi mode indicator support for prompts via `prompt.mode_indicator` config (#23)

### Changed

- `arf history import` now skips duplicate entries by default (anti-join on command text and timestamp). Use `--import-duplicates` to import all entries regardless (#52)
- `arf history import --from arf` now supports importing from unified export files (files other than `r.db` or `shell.db`)
  - Use `--r-table` and `--shell-table` to specify custom table names
- History browser now displays timestamps in local time instead of UTC (#53)
- Vi mode prompt indicators now have sensible defaults: `[I]` for insert mode (LightGreen) and `[N]` for normal mode (LightYellow) (#45)
- **BREAKING:** Configuration structure reorganized — the `[reprex]` section has been split into `[startup.mode]` and `[mode.reprex]` for better semantic organization (#27)
- **BREAKING:** `editor.autosuggestion` config key renamed to `editor.auto_suggestions` for naming consistency with `auto_match` (#48)
- **BREAKING:** `completion.function_paren_check_limit` config key renamed to `completion.auto_paren_limit` (#48)
- **BREAKING:** `editor.mode` is now a typed enum accepting only `"emacs"` or `"vi"` (#48)
- Improved JSON Schema for color properties with proper `oneOf` typing (named string, `{ Fixed: N }`, `{ Rgb: [r, g, b] }`) (#48)

#### Migration Guide

If you have a custom configuration file from 0.1.x, apply the following changes:

| 0.1.x key | 0.2.0 key |
|-----------|-----------|
| `reprex.enabled` | `startup.mode.reprex` |
| `reprex.autoformat` | `startup.mode.autoformat` |
| `reprex.comment` | `mode.reprex.comment` |
| `editor.autosuggestion` | `editor.auto_suggestions` |
| `completion.function_paren_check_limit` | `completion.auto_paren_limit` |
| `editor.mode = "vim"` | `editor.mode = "vi"` |

**Before (0.1.x):**

```toml
[reprex]
enabled = false
comment = "#> "
autoformat = false

[editor]
autosuggestion = true

[completion]
function_paren_check_limit = 50
```

**After (0.2.0):**

```toml
# Initial mode settings (can be toggled at runtime via :reprex, :autoformat)
[startup.mode]
reprex = false
autoformat = false

# Static reprex configuration (not changeable at runtime)
[mode.reprex]
comment = "#> "

[editor]
auto_suggestions = true

[completion]
auto_paren_limit = 50
```

### Fixed

- **Windows:** Fixed garbled error message display caused by CRLF line endings in R output (#56)
- **Windows:** Fixed multiline input causing "invalid token" error due to CRLF newlines from reedline (#57)
- Flush stdout after print in `r_write_console_ex` to prevent output buffering issues (#44)
- Use display-width-aware truncation for "Copied" feedback message (#41)
- Mouse wheel scroll now moves cursor in history browser (#40)
- Use display-width-aware text utilities for correct CJK character rendering (#39)
- Correct sponge delay semantics in `history_forget` (#37)
- Windows: Manually source `.Rprofile` etc. after R initialization (#20)
- Use intermediate pointer cast for signal handlers (#16)

## [0.1.1] - 2026-01-31

### Added

- Experimental animated prompt spinner with color support (#9)

### Fixed

- Windows: Enable UTF-8 support for non-ASCII input (#6)
- Improve spinner shutdown responsiveness (#11)
- Add explicit property definitions to ColorsConfig schema (#10)

## [0.1.0] - 2026-01-29

Initial release.
