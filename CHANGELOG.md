# Changelog

## [Unreleased]

### Added

- Experimental `arf history import` subcommand for importing history from radian, R, or another arf database (#31)
- Enhanced `:info` meta command with pager view, clipboard copy, and path masking (#29)

## [0.2.0-beta.1] - 2026-02-01

### Added

- Vi mode indicator support for prompts via `prompt.mode_indicator` config (#23)

### Changed

- **BREAKING:** Configuration structure reorganized - the `[reprex]` section has been split into `[startup.mode]` and `[mode.reprex]` for better semantic organization.

#### Migration Guide

The `[reprex]` section mapping:

- `reprex.enabled` → `startup.mode.reprex`
- `reprex.autoformat` → `startup.mode.autoformat`
- `reprex.comment` → `mode.reprex.comment`

If you have a custom configuration file, update your `[reprex]` section as follows:

**Before (0.1.x):**

```toml
[reprex]
enabled = false
comment = "#> "
autoformat = false
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
