# Editor Integration

arf can be used as the R terminal in code editors that support custom terminal programs.

## VS Code (vscode-R)

The [vscode-R](https://github.com/REditorSupport/vscode-R) extension provides R language support in VS Code, including the ability to send code from the editor to an R terminal. To use arf as the R terminal, configure the following settings.

### Settings

Set `r.rterm` to `"arf"` for your platform, and enable `r.bracketedPaste`:

```json
{
    "r.rterm.linux": "arf",
    "r.bracketedPaste": true
}
```

The platform-specific `r.rterm` settings are `r.rterm.linux`, `r.rterm.mac`, and `r.rterm.windows`.

Without `r.bracketedPaste`, vscode-R sends code line-by-line to the terminal. This can cause issues with arf's auto-match feature (automatic bracket/quote completion), because each line is processed as individual keystrokes. With bracketed paste enabled, code is sent as a single unit, avoiding interference from auto-match.

> [!NOTE]
> This is the same recommendation as for radian. The `r.bracketedPaste` setting is disabled by default in vscode-R because the standard R terminal does not support it.

> [!WARNING]
> On Windows, vscode-R has a [known bug](https://github.com/REditorSupport/vscode-R/issues/1590) where bracketed paste mode is silently disabled. When `r.bracketedPaste` is `true`, vscode-R still sends code line-by-line on Windows, which can cause issues with auto-match. As a workaround, set `r.bracketedPaste` to `false` and consider [disabling auto-match](configuration.md) (`auto_match = false` under `[editor]` in `arf.toml`) if you encounter problems when sending code from the editor.

## Zed (zed-r)

The [zed-r](https://github.com/ocsmit/zed-r) extension provides R language support in Zed. You can use arf as the R terminal by configuring a task.

### Setting Up an R Terminal Task

Add the following to your `tasks.json`:

```json
[
    {
        "label": "R Terminal (arf)",
        "command": "arf",
        "cwd": "$ZED_WORKTREE_ROOT",
        "use_new_terminal": true
    }
]
```

You can bind this task to a keyboard shortcut in your `keymap.json`:

```json
{
    "context": "Workspace",
    "bindings": {
        "ctrl-2": [
            "task::Spawn",
            { "task_name": "R Terminal (arf)", "reveal_target": "dock" }
        ]
    }
}
```

### Bracketed Paste

No special configuration is needed for bracketed paste in Zed. Zed's terminal handles paste mode automatically.

## Migrating from radian

If you are currently using radian with vscode-R, the migration to arf is straightforward:

1. Change `r.rterm` from the radian path to the arf path.
2. Keep `r.bracketedPaste` set to `true` (same as radian).
3. No other vscode-R settings need to change.

For Zed users, replace `"radian"` with `"arf"` in your task's `command` field.

Note that arf uses its own configuration file (`arf.toml`) instead of `.radian_profile`. See the [Configuration](configuration.md) documentation for details on configuring arf.

### Importing History

You can import your radian command history into arf:

```sh
# Preview what would be imported (dry run)
arf history import --from radian --dry-run

# Import from radian (default: ~/.radian_history)
arf history import --from radian
```

> [!CAUTION]
> This feature is experimental. Always back up your history files before importing. Importing the same file multiple times will create duplicate entries.
