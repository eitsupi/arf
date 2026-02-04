# Changelog

## [Unreleased]

### Added

- Experimental `arf history import` subcommand for importing history from radian, R, or another arf database (#31)
- Enhanced `:info` meta command with pager view, clipboard copy, and path masking (#29)
- Vi mode indicator support for prompts via `prompt.mode_indicator` config (#23)

### Changed

- Vi mode prompt indicators now have sensible defaults: `[I]` for insert mode (LightGreen) and `[N]` for normal mode (LightYellow)
- **BREAKING:** Configuration structure reorganized - the `[reprex]` section has been split into `[startup.mode]` and `[mode.reprex]` for better semantic organization.
- **BREAKING:** `editor.autosuggestion` config key renamed to `editor.auto_suggestions` for naming consistency with `auto_match`
- **BREAKING:** `completion.function_paren_check_limit` config key renamed to `completion.auto_paren_limit`
- **BREAKING:** `editor.mode` is now a typed enum accepting only `"emacs"` or `"vi"`
- Improved JSON Schema for color properties with proper `oneOf` typing (named string, `{ Fixed: N }`, `{ Rgb: [r, g, b] }`)

#### Migration Guide

If you have a custom configuration file from 0.1.x, apply the following changes:

| 0.1.x key | 0.2.0 key |
|-----------|-----------|
| `reprex.enabled` | `startup.mode.reprex` |
| `reprex.autoformat` | `startup.mode.autoformat` |
| `reprex.comment` | `mode.reprex.comment` |
| `editor.autosuggestion` | `editor.auto_suggestions` |
| `completion.function_paren_check_limit` | `completion.auto_paren_limit` |

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

- Windows: Manually source `.Rprofile` etc. after R initialization (#20)

## [0.1.1] - 2026-01-31

### Added

- Experimental animated prompt spinner with color support (#9)

### Fixed

- Windows: Enable UTF-8 support for non-ASCII input (#6)
- Improve spinner shutdown responsiveness (#11)
- Add explicit property definitions to ColorsConfig schema (#10)

## [0.1.0] - 2026-01-29

Initial release.
