# Editor Integration

arf can be used as the R terminal in code editors that support custom terminal programs.

## VS Code (vscode-R)

The [vscode-R](https://github.com/REditorSupport/vscode-R) extension provides R language support in VS Code, including the ability to send code from the editor to an R terminal. To use arf as the R terminal, configure the following settings.

### Setting the R Terminal Path

Set `r.rterm` to the path of the arf binary for your platform. Open VS Code settings (`Ctrl+,` or `Cmd+,`) and search for `r.rterm`, then set the appropriate platform-specific entry:

- **Linux**: `r.rterm.linux`
- **macOS**: `r.rterm.mac`
- **Windows**: `r.rterm.windows`

For example, if arf is installed via `cargo install`:

```json
{
    "r.rterm.linux": "${userHome}/.cargo/bin/arf",
    "r.rterm.mac": "${userHome}/.cargo/bin/arf",
    "r.rterm.windows": "${userHome}\\.cargo\\bin\\arf.exe"
}
```

If arf is already in your `PATH`, you can simply set:

```json
{
    "r.rterm.linux": "arf"
}
```

### Enabling Bracketed Paste

You should enable `r.bracketedPaste` for arf:

```json
{
    "r.bracketedPaste": true
}
```

Without this setting, vscode-R sends code line-by-line to the terminal. This can cause issues with arf's auto-match feature (automatic bracket/quote completion), because each line is processed as individual keystrokes. With bracketed paste enabled, code is sent as a single unit, avoiding interference from auto-match.

> **Note**: This is the same recommendation as for radian. The `r.bracketedPaste` setting is disabled by default in vscode-R because the standard R terminal does not support it.

### Complete Example

A minimal `settings.json` for using arf with vscode-R on Linux:

```json
{
    "r.rterm.linux": "arf",
    "r.bracketedPaste": true
}
```

## Zed (zed-r)

The [zed-r](https://github.com/ocsmit/zed-r) extension provides R language support in Zed. You can use arf as the R terminal by configuring a task.

### Setting Up an R Terminal Task

Add the following to your `tasks.json` (open with `Cmd+Shift+P` or `Ctrl+Shift+P` and select "zed: open tasks"):

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
