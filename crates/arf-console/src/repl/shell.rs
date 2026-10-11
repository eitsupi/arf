//! Shell command execution and process management.

use crate::external::rig::{self, RigError};
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::process::{Command, Stdio};

use super::{arf_eprintln, arf_println};

/// Execute a shell command with direct stdin/stdout connection.
///
/// This uses inherited stdio so that:
/// - Interactive programs work (vim, less, python REPL)
/// - Commands that read stdin work (cat, read)
/// - Output streams in real-time
pub fn execute_shell_command(cmd: &str) {
    #[cfg(unix)]
    let result = {
        // Use user's default shell from $SHELL, fall back to /bin/sh
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        Command::new(&shell)
            .arg("-c")
            .arg(cmd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
    };

    #[cfg(windows)]
    let result = Command::new("cmd")
        .arg("/c")
        .arg(cmd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();

    match result {
        Ok(mut child) => {
            // Wait for the child process to complete
            if let Err(e) = child.wait() {
                arf_eprintln!("Failed to wait for command: {}", e);
            }
        }
        Err(e) => {
            arf_eprintln!("Failed to execute command: {}", e);
        }
    }
}

/// Prompt the user for confirmation (y/n).
///
/// Returns true if the user confirms, false otherwise.
pub fn confirm_action(prompt: &str) -> bool {
    print!("{} [y/N]: ", prompt);
    let _ = io::stdout().flush();

    let mut input = String::new();
    match io::stdin().read_line(&mut input) {
        Ok(_) => {
            let response = input.trim().to_lowercase();
            response == "y" || response == "yes"
        }
        Err(_) => false,
    }
}

/// Environment variables whose R-specific values must not leak across `:switch`.
///
/// Values inherited at arf startup are restored for all variables except the
/// names in `ALWAYS_REMOVE_ENV_VARS`. Values absent at startup are removed so
/// that the new R process can compute them afresh.
/// The comments record where each value comes from when the user did not
/// supply one, which is what makes it version-specific.
const R_VERSION_ENV_VARS: &[&str] = &[
    // Always removed, whatever its origin; see `ALWAYS_REMOVE_ENV_VARS`.
    "R_HOME",
    // Always removed, whatever its origin; see `ALWAYS_REMOVE_ENV_VARS`.
    "LD_LIBRARY_PATH",
    // Always removed so rig reevaluates project activation on the new startup.
    "RVENV",
    "RVENV_UNSYNCED",
    #[cfg(target_os = "macos")]
    // Restored from the startup snapshot before the next R target is selected.
    "DYLD_LIBRARY_PATH",
    // Read from the old R_HOME/etc/Renviron.
    "R_LIBS_USER",
    "R_LIBS_SITE",
    // Not set by R itself; may be supplied by the user or the session.
    "R_LIBS",
    "R_SYSTEM_ABI",
    // Set from the old R_HOME/bin/R wrapper script.
    "R_DOC_DIR",
    "R_SHARE_DIR",
    "R_INCLUDE_DIR",
];

/// Environment variables removed before a version switch whatever the startup
/// snapshot holds.
///
/// `R_HOME` is removed because arf always sets it from the R version it
/// resolved, so any inherited value is meaningless and restoring it would be
/// wrong. `LD_LIBRARY_PATH` keeps its existing removal policy. Rig's
/// activation markers are removed so the new startup reevaluates the project.
///
/// TODO: Let `LD_LIBRARY_PATH` follow the same restore-or-remove rule as the
/// rest of `R_VERSION_ENV_VARS`, so a value the user set for unrelated
/// libraries survives a switch instead of being dropped. That changes what
/// users observe, so it belongs in a change of its own.
const ALWAYS_REMOVE_ENV_VARS: &[&str] = &["LD_LIBRARY_PATH", "R_HOME", "RVENV", "RVENV_UNSYNCED"];

#[derive(Default)]
struct EnvChanges {
    restored: Vec<(&'static str, OsString)>,
    removed: Vec<&'static str>,
}

/// Return the environment changes needed before restarting.
///
/// The startup snapshot is injected so this policy can be tested without
/// depending on the process environment. Variables in
/// `ALWAYS_REMOVE_ENV_VARS` are always removed for their distinct policies:
/// `R_HOME` is always set by arf from the resolved R version, while
/// `LD_LIBRARY_PATH` remains removed to preserve current behaviour even though
/// the carrier preserves a user-set startup value across re-execs.
fn env_changes_for_switch<F>(version: Option<&str>, mut startup_value: F) -> EnvChanges
where
    F: FnMut(&str) -> Option<OsString>,
{
    let mut changes = EnvChanges::default();

    if version.is_none() {
        return changes;
    }

    for &var in R_VERSION_ENV_VARS {
        if ALWAYS_REMOVE_ENV_VARS.contains(&var) {
            changes.removed.push(var);
        } else if let Some(value) = startup_value(var) {
            changes.restored.push((var, value));
        } else {
            changes.removed.push(var);
        }
    }

    changes
}

/// Build the command used to restart the process with the requested changes.
///
/// Using `Command::env_remove()` (rather than mutating the current process's
/// own environment before spawning/exec'ing) keeps this scoped to the child:
/// on the non-Unix spawn path the parent process stays alive after this call
/// returns, so mutating its environment directly would leak into it.
fn build_restart_command(
    exe: &std::path::Path,
    args: &[OsString],
    changes: &EnvChanges,
    startup_env_carrier: &str,
) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args);
    cmd.env(crate::STARTUP_ENV_CARRIER, startup_env_carrier);
    #[cfg(unix)]
    if let Some(fd) = crate::pid_file::restart_fd_carrier() {
        cmd.env(crate::pid_file::RESTART_PID_FD_ENV, fd);
    }
    for (var, value) in &changes.restored {
        cmd.env(var, value);
    }
    for var in &changes.removed {
        cmd.env_remove(var);
    }
    cmd
}

/// Print the environment changes that will affect the restarted process.
fn print_env_changes(changes: &EnvChanges) {
    let removed: Vec<&str> = changes
        .removed
        .iter()
        .copied()
        .filter(|var| std::env::var_os(var).is_some())
        .collect();

    if changes.restored.is_empty() && removed.is_empty() {
        return;
    }

    let mut details = Vec::new();
    if !changes.restored.is_empty() {
        details.push(format!(
            "restored: {}",
            changes
                .restored
                .iter()
                .map(|(var, _)| *var)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !removed.is_empty() {
        details.push(format!("removed: {}", removed.join(", ")));
    }

    arf_println!(
        "Environment variables for the R version switch: {}",
        details.join("; ")
    );
}

/// Restart the process, optionally with a new R version.
///
/// This function uses exec() to replace the current process with a new instance.
/// If version is specified, it resolves the R_HOME using rig before restarting.
///
/// This function only returns if exec fails.
pub fn restart_process(version: Option<&str>) {
    // If a version is specified, validate it using rig before restarting
    if let Some(ver) = version {
        if let Err(message) = validate_rig_for_switch_with(rig::rig_available) {
            for line in message.lines() {
                arf_eprintln!("{}", line);
            }
            return;
        }

        // Validate the version exists before restarting
        match rig::resolve_version(ver) {
            Ok(resolved) => {
                log::info!(
                    "Switching to R version {} ({})",
                    resolved.version,
                    resolved.r_home
                );
            }
            Err(e) => {
                arf_eprintln!("Error: {}", e);
                return;
            }
        }
    }

    // Get the current executable
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            arf_eprintln!("Error: Failed to get current executable: {}", e);
            return;
        }
    };

    // Get command-line arguments (skip the program name, we'll use current_exe instead)
    let mut args: Vec<OsString> = crate::normalized_args();

    if let Some(v) = &version {
        args = args_with_r_version(args, v);
    }

    let changes = env_changes_for_switch(version, crate::startup_env_value);
    print_env_changes(&changes);
    let startup_env_carrier = crate::startup_env_carrier();

    // Build the command
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let _pid_fd_guard = match crate::pid_file::prepare_pid_fd_for_exec() {
            Some(Ok(guard)) => Some(guard),
            Some(Err(error)) => {
                arf_eprintln!("Error: Failed to prepare PID file for restart: {error}");
                return;
            }
            None => None,
        };
        let mut cmd = build_restart_command(&exe, &args, &changes, &startup_env_carrier);

        // exec() replaces the current process - this should not return
        let err = cmd.exec();
        arf_eprintln!("Error: Failed to restart: {}", err);
    }

    #[cfg(not(unix))]
    {
        // A spawned replacement cannot safely share the parent's PID file.
        // Stop serving first, then relinquish the file; on failure the old
        // file is left alone and no child is started.
        crate::ipc::stop_server();
        if let Some(pid_path) = crate::pid_file::initial_pid_file_path()
            && let Err(e) = crate::pid_file::relinquish_pid_file_for_restart(&pid_path)
        {
            arf_eprintln!("Error: {}", e);
            std::process::exit(1);
        }
        // On non-Unix platforms, spawn a new process and wait for it to exit.
        // Unlike Unix exec(), this doesn't replace the current process, so we
        // wait for the child and then exit with its status code. This keeps the
        // parent alive to hold the terminal session, preventing the shell from
        // reclaiming control.
        match build_restart_command(&exe, &args, &changes, &startup_env_carrier)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(mut child) => match child.wait() {
                Ok(status) => std::process::exit(status.code().unwrap_or(1)),
                Err(e) => {
                    arf_eprintln!("Error: Failed to wait for restarted process: {}", e);
                    std::process::exit(1);
                }
            },
            Err(e) => {
                arf_eprintln!("Error: Failed to restart: {}", e);
                std::process::exit(1);
            }
        }
    }
}

fn validate_rig_for_switch_with<FAvailable>(rig_available: FAvailable) -> Result<(), String>
where
    FAvailable: FnOnce() -> Result<(), RigError>,
{
    rig_available().map_err(|error| match error {
        RigError::NotInstalled => {
            "Error: rig is not installed. Cannot switch R versions.\nInstall rig from https://github.com/r-lib/rig".to_string()
        }
        RigError::CommandFailed(reason) => format!(
            "Error: rig is installed but failed while checking availability: {reason}\nCannot switch R versions until rig is working."
        ),
        error => format!(
            "Error: Could not check whether rig is available: {error}\nCannot switch R versions until rig is working."
        ),
    })
}

/// Filter explicit R source selections before adding the new R version.
/// Preserve all arguments after `--` because they belong to the user program.
fn filter_r_version_args(args: Vec<OsString>) -> Vec<OsString> {
    let mut result = Vec::new();
    let mut index = 0;

    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            result.extend(args[index..].iter().cloned());
            break;
        }

        if arg == "--with-r-version" || arg == "--r-provider" {
            index = (index + 2).min(args.len());
            continue;
        }

        if arg_has_ascii_prefix(arg, b"--with-r-version=")
            || arg_has_ascii_prefix(arg, b"--r-provider=")
        {
            index += 1;
            continue;
        }

        result.push(arg.clone());
        index += 1;
    }

    result
}

fn args_with_r_version(args: Vec<OsString>, version: &str) -> Vec<OsString> {
    let mut args = filter_r_version_args(args);
    let separator = args
        .iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len());
    args.splice(
        separator..separator,
        [OsString::from("--with-r-version"), OsString::from(version)],
    );
    args
}

fn arg_has_ascii_prefix(arg: &OsStr, prefix: &[u8]) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        arg.as_bytes().starts_with(prefix)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        arg.encode_wide()
            .take(prefix.len())
            .eq(prefix.iter().copied().map(u16::from))
    }
    #[cfg(not(any(unix, windows)))]
    {
        arg.to_str()
            .is_some_and(|arg| arg.as_bytes().starts_with(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    const TEST_STARTUP_ENV_CARRIER: &str = r#"{"version":1,"variables":{}}"#;

    #[test]
    fn command_failed_rig_switch_error_preserves_reason_without_install_guidance() {
        let error = validate_rig_for_switch_with(|| {
            Err(RigError::CommandFailed("permission denied".to_string()))
        })
        .expect_err("a failed rig command should reject switching");

        assert!(error.contains("permission denied"));
        assert!(!error.contains("Install rig"));
    }

    #[test]
    fn build_restart_command_restores_a_startup_value() {
        let changes = env_changes_for_switch(Some(r"4.0"), |var| {
            (var == "R_LIBS_USER").then(|| OsString::from(r"/user/r/library"))
        });
        let cmd = build_restart_command(
            std::path::Path::new(r"arf"),
            &[],
            &changes,
            TEST_STARTUP_ENV_CARRIER,
        );

        let restored = cmd
            .get_envs()
            .find(|(key, _)| *key == OsStr::new(r"R_LIBS_USER"))
            .map(|(_, value)| value);
        assert_eq!(restored, Some(Some(OsStr::new(r"/user/r/library"))));
    }

    #[test]
    fn build_restart_command_restores_r_libs_from_startup_snapshot() {
        let changes = env_changes_for_switch(Some(r"4.0"), |var| {
            (var == "R_LIBS").then(|| OsString::from(r"/user/r/libs"))
        });
        let cmd = build_restart_command(
            std::path::Path::new(r"arf"),
            &[],
            &changes,
            TEST_STARTUP_ENV_CARRIER,
        );

        let restored = cmd
            .get_envs()
            .find(|(key, _)| *key == OsStr::new(r"R_LIBS"))
            .map(|(_, value)| value);
        assert_eq!(restored, Some(Some(OsStr::new(r"/user/r/libs"))));
    }

    #[test]
    fn build_restart_command_removes_r_libs_absent_from_snapshot() {
        let changes = env_changes_for_switch(Some(r"4.0"), |_| None);
        let cmd = build_restart_command(
            std::path::Path::new(r"arf"),
            &[],
            &changes,
            TEST_STARTUP_ENV_CARRIER,
        );

        assert!(
            cmd.get_envs()
                .any(|(key, value)| key == OsStr::new(r"R_LIBS") && value.is_none())
        );
    }

    #[test]
    fn build_restart_command_always_removes_ld_library_path() {
        let changes = env_changes_for_switch(Some(r"4.0"), |var| {
            (var == "LD_LIBRARY_PATH").then(|| OsString::from(r"/old/r/lib"))
        });
        let cmd = build_restart_command(
            std::path::Path::new(r"arf"),
            &[],
            &changes,
            TEST_STARTUP_ENV_CARRIER,
        );

        assert!(
            cmd.get_envs()
                .any(|(key, value)| key == OsStr::new(r"LD_LIBRARY_PATH") && value.is_none())
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn build_restart_command_restores_dyld_library_path_from_startup_snapshot() {
        let changes = env_changes_for_switch(Some(r"4.0"), |var| {
            (var == "DYLD_LIBRARY_PATH").then(|| OsString::from(r"/user/lib"))
        });
        let cmd = build_restart_command(
            std::path::Path::new(r"arf"),
            &[],
            &changes,
            TEST_STARTUP_ENV_CARRIER,
        );

        let restored = cmd
            .get_envs()
            .find(|(key, _)| *key == OsStr::new(r"DYLD_LIBRARY_PATH"))
            .map(|(_, value)| value);
        assert_eq!(restored, Some(Some(OsStr::new(r"/user/lib"))));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn build_restart_command_removes_dyld_library_path_absent_from_snapshot() {
        let changes = env_changes_for_switch(Some(r"4.0"), |_| None);
        let cmd = build_restart_command(
            std::path::Path::new(r"arf"),
            &[],
            &changes,
            TEST_STARTUP_ENV_CARRIER,
        );

        assert!(
            cmd.get_envs()
                .any(|(key, value)| key == OsStr::new(r"DYLD_LIBRARY_PATH") && value.is_none())
        );
    }

    #[test]
    fn build_restart_command_always_removes_r_home_from_snapshot() {
        let changes = env_changes_for_switch(Some(r"4.0"), |var| {
            (var == "R_HOME").then(|| OsString::from(r"/old/r"))
        });
        let cmd = build_restart_command(
            std::path::Path::new(r"arf"),
            &[],
            &changes,
            TEST_STARTUP_ENV_CARRIER,
        );

        assert!(
            cmd.get_envs()
                .any(|(key, value)| key == OsStr::new(r"R_HOME") && value.is_none())
        );
        assert!(!changes.restored.iter().any(|(key, _)| *key == "R_HOME"));
    }

    #[test]
    fn build_restart_command_removes_rig_activation_markers() {
        let changes = env_changes_for_switch(Some(r"4.0"), |var| match var {
            "RVENV" => Some(OsString::from(r"/project/.rvenv")),
            "RVENV_UNSYNCED" => Some(OsString::from("1")),
            _ => None,
        });
        let cmd = build_restart_command(
            std::path::Path::new(r"arf"),
            &[],
            &changes,
            TEST_STARTUP_ENV_CARRIER,
        );

        for marker in ["RVENV", "RVENV_UNSYNCED"] {
            assert!(
                cmd.get_envs()
                    .any(|(key, value)| key == OsStr::new(marker) && value.is_none())
            );
            assert!(!changes.restored.iter().any(|(key, _)| *key == marker));
        }
    }

    #[test]
    fn version_none_registers_no_environment_changes() {
        let changes = env_changes_for_switch(None, |_| {
            panic!("restart without a version must not inspect the startup snapshot")
        });
        let cmd = build_restart_command(
            std::path::Path::new(r"arf"),
            &[],
            &changes,
            TEST_STARTUP_ENV_CARRIER,
        );

        assert!(changes.restored.is_empty());
        assert!(changes.removed.is_empty());
        assert!(
            cmd.get_envs().all(|(key, _)| {
                key != OsStr::new("RVENV") && key != OsStr::new("RVENV_UNSYNCED")
            })
        );
        assert_eq!(
            cmd.get_envs()
                .find(|(key, _)| *key == OsStr::new(crate::STARTUP_ENV_CARRIER))
                .map(|(_, value)| value),
            Some(Some(OsStr::new(TEST_STARTUP_ENV_CARRIER)))
        );
    }

    #[test]
    fn build_restart_command_sets_carrier_for_switch() {
        let changes = env_changes_for_switch(Some(r"4.0"), |_| None);
        let cmd = build_restart_command(
            std::path::Path::new(r"arf"),
            &[],
            &changes,
            TEST_STARTUP_ENV_CARRIER,
        );

        assert_eq!(
            cmd.get_envs()
                .find(|(key, _)| *key == OsStr::new(crate::STARTUP_ENV_CARRIER))
                .map(|(_, value)| value),
            Some(Some(OsStr::new(TEST_STARTUP_ENV_CARRIER)))
        );
    }

    #[test]
    fn switching_replaces_explicit_r_sources_before_user_arguments() {
        let args = vec![
            OsString::from("--quiet"),
            OsString::from("--with-r-version"),
            OsString::from("4.3"),
            OsString::from("--r-provider=rig-project"),
            OsString::from("--r-provider"),
            OsString::from("pixi"),
            OsString::from("--with-r-version=4.4"),
            OsString::from("--"),
            OsString::from("--r-provider"),
            OsString::from("script-argument"),
        ];

        assert_eq!(
            args_with_r_version(args, "4.5"),
            vec![
                OsString::from("--quiet"),
                OsString::from("--with-r-version"),
                OsString::from("4.5"),
                OsString::from("--"),
                OsString::from("--r-provider"),
                OsString::from("script-argument"),
            ]
        );
    }

    #[cfg(unix)]
    #[test]
    fn switching_preserves_non_utf8_arguments_after_separator() {
        use std::os::unix::ffi::OsStringExt;

        let user_arg = OsString::from_vec(vec![b'R', b'-', 0xff]);
        let args = vec![OsString::from("--"), user_arg.clone()];

        let switched = args_with_r_version(args, "4.5");

        assert_eq!(
            switched,
            vec![
                OsString::from("--with-r-version"),
                OsString::from("4.5"),
                OsString::from("--"),
                user_arg,
            ]
        );
    }
}
