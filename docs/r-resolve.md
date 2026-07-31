# `arf r resolve`

`arf r resolve` is a machine-facing API for editor extensions and other external tools that need to know which R installation arf would use before starting R.

> [!WARNING]
> `arf r resolve` is experimental. The command name, JSON descriptor, and exit codes may change in future versions.

## Overview

An editor extension and arf must agree on which R installation to use. `arf r resolve` lets the editor ask arf rather than reimplementing arf's R source selection logic. The selection rules themselves are documented in [R Source Overrides](configuration.md#r-source-overrides) and [R Source Precedence](configuration.md#r-source-precedence); this page documents the interface.

To ask arf which R installation it would use without starting R, run:

```bash
arf r resolve
```

`arf r resolve` accepts the same `--r-home`, `--with-r-version`, `--no-r-source-overrides`, and `--config` options as the startup path. On success it always emits JSON; there is no `--json` flag. Output is pretty-printed when stdout is a terminal and compact when piped, so consumers do not need a format flag. For example:

```json
{
  "schema_version": 1,
  "resolved": true,
  "cwd": "/project",
  "target": {
    "r_home": "/opt/R/4.5.2/lib/R",
    "r_binary": "/opt/R/4.5.2/lib/R/bin/R",
    "resolved_version": "4.5.2"
  },
  "resolver": { "name": "arf", "version": "0.4.4" },
  "selected_by": {
    "kind": "version_request",
    "requested_r_home": null,
    "requested_version": "4.5",
    "source": {
      "kind": "project_file",
      "name": null,
      "path": "/project/rproject.toml",
      "format": "toml",
      "key": "project.r_version"
    }
  },
  "provider": "rig",
  "diagnostics": []
}
```

## JSON Descriptor

Every key is always present. A value is `null` when it does not apply, so consumers can rely on the key set being stable. `resolved` is `false` and `target` is `null` exactly when no R installation could be found. That is not an error: the command still exits 0, because normal arf startup also continues in that state with R evaluation unavailable.

`resolver` names the tool that produced the descriptor, so resolution could later move behind a separate tool without breaking readers. `provider` identifies the mechanism that located the installation; current values are `rig`, `path`, and `explicit_path`. `resolved_version` is deliberately not called `r_version`: it is a prediction made before R starts, unlike the versions reported from a running session by the IPC layer. See [IPC session information](ipc.md#arf-ipc-session--get-session-info) for those runtime values.

### `selected_by` and `source`

`selected_by` separates what was matched from where that condition came from. Its `kind` values are:

| Value | Meaning |
|-------|---------|
| `r_home` | An explicit R_HOME path was selected. |
| `version_request` | A requested R version was selected, from the command line, environment, or a project file. |
| `default` | No explicit request selected the R source; configuration or the built-in default was used. |

The `source.kind` values identify the origin of that condition:

| Value | Meaning |
|-------|---------|
| `command_line_argument` | `--r-home` or `--with-r-version`. |
| `environment_variable` | `ARF_R_HOME` or `ARF_R_VERSION`. |
| `project_file` | A project file, with its path, format, and optional key. |
| `configuration_file` | The loaded `arf.toml`, with its path, format, and key. |
| `built_in_default` | arf's built-in default when no configuration file setting applies. |

Both enum sets may grow, so clients must accept unknown values. The source fields carry the specific name (`--r-home`, `ARF_R_VERSION`, or a file path plus key) so a caller can point the user at the exact thing to change. The nullable `requested_r_home`, `requested_version`, `name`, `path`, `format`, and `key` fields remain present in every descriptor.

## Diagnostics

`diagnostics` is a list of `{code, severity, message, path}` objects. Codes are an open enum: clients must not reject unknown codes, and the set may grow. `message` is for display only and must not be used for machine classification. The codes currently produced are:

| Code | Meaning |
|------|---------|
| `config.read_failed` | The configuration file could not be read. |
| `config.parse_failed` | The configuration file could not be parsed. |
| `r_discovery.failed` | No R could be discovered from PATH or the default installation paths. This accompanies `resolved: false`. |
| `r_source_override.provider_unsupported` | An R source override provider is unsupported. |
| `r_source_override.value_invalid` | An R source override value is invalid. |
| `r_source_override.fallback` | R source overrides fell back to the configured startup source. |
| `r_source_override.resolution_failed` | An R source override could not be resolved. |
| `r_source_override.rig_unavailable` | rig was unavailable while evaluating an override. |
| `r_source_override.version_not_installed` | A requested override version is not installed. |

## Resolution Behavior

Resolution mirrors normal startup rather than being stricter. A configuration file that fails to load still falls back to defaults and exits 0 with a diagnostic, and a version request that a project file merely suggests does the same when it cannot be satisfied. A version requested explicitly on the command line or through `ARF_R_HOME` / `ARF_R_VERSION` is an error when it cannot be satisfied, because startup refuses to continue in that case too. Requests that resolve successfully behave the same way whatever their source. Reporting a different R than arf would actually use would defeat the purpose of the command.

## Exit Codes

Exit codes match `arf ipc`: `0` means success, including `resolved: false`; `2` means invalid invocation; and `4` means internal failure.

Errors raised after the arguments parse successfully are JSON on stderr, in the same shape used by `arf ipc`. Argument-parsing failures — an unknown flag, a missing value, or `--r-home` combined with `--with-r-version` — are reported by the argument parser as plain text and also exit `2`, so a client that needs to distinguish them must tolerate a non-JSON stderr at that exit code.
