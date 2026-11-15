use super::super::support::{PROMPT, Terminal, run_case_with, wait_for_path_absent};
use super::session_metadata_path;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
fn custom_bind_path(root: &Path) -> String {
    root.join("arf-tui-custom.sock").display().to_string()
}

#[cfg(windows)]
fn custom_bind_path(root: &Path) -> String {
    let unique = root
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "session".into());
    format!(r"\\.\pipe\arf-tui-{unique}")
}

fn assert_pid_file(terminal: &Terminal, pid_path: &Path) -> Result<PathBuf> {
    ensure!(
        pid_path.is_file(),
        "PID file was not ready by the first prompt: {}",
        pid_path.display()
    );
    let pid = terminal.pid().context("missing arf PID")?;
    ensure!(
        fs::read_to_string(pid_path)?.trim() == pid.to_string(),
        "PID file does not identify this process"
    );
    let metadata = session_metadata_path(terminal)?;
    ensure!(
        metadata.is_file(),
        "session metadata was not written: {}",
        metadata.display()
    );
    Ok(metadata)
}

#[test]
fn custom_ipc_bind_uses_cli_transport_and_cleans_up_metadata() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let bind_path = custom_bind_path(temp.path());
    let mut metadata_path = None;
    run_case_with(
        Terminal::builder("ipc-custom-bind").args([
            "--with-ipc",
            "--ipc-eval-unrestricted",
            "--ipc-bind",
            bind_path.as_str(),
        ]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let metadata = session_metadata_path(terminal)?;
            let metadata_json: Value = serde_json::from_str(&fs::read_to_string(&metadata)?)?;
            ensure!(
                metadata_json["socket_path"] == bind_path,
                "session metadata has wrong bind path: {metadata_json}"
            );
            let session = terminal.start_ipc(&["session"])?.finish()?;
            ensure!(
                session["socket_path"] == bind_path,
                "session response has wrong bind path: {session}"
            );
            let response = terminal
                .start_ipc(&["eval", "1 + 1", "--timeout", "10000"])?
                .finish()?;
            ensure!(
                response["value"] == "[1] 2" && response["error"].is_null(),
                "custom bind IPC evaluation failed: {response}"
            );
            terminal.submit(
                "cat('CUSTOM_BIND_REPL_READY\\n')",
                "CUSTOM_BIND_REPL_READY",
                PROMPT,
            )?;
            metadata_path = Some(metadata);
            terminal.key("Ctrl+D")?;
            ensure!(
                terminal.wait_for_exit()? == 0,
                "custom-bind Ctrl+D did not exit successfully"
            );
            Ok(())
        },
    )?;
    wait_for_path_absent(
        metadata_path
            .as_deref()
            .context("custom-bind metadata path was not captured")?,
    )?;
    #[cfg(unix)]
    ensure!(
        !Path::new(&bind_path).exists(),
        "custom Unix IPC socket remained after interactive exit: {bind_path}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn custom_ipc_cleanup_preserves_replacement_socket() -> Result<()> {
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::net::UnixListener;

    let temp = tempfile::tempdir()?;
    let bind_path = custom_bind_path(temp.path());
    let mut replacement_listener = None;
    run_case_with(
        Terminal::builder("ipc-custom-bind-replacement").args([
            "--with-ipc",
            "--ipc-bind",
            bind_path.as_str(),
        ]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            fs::remove_file(&bind_path)?;
            replacement_listener = Some(UnixListener::bind(&bind_path)?);
            terminal.key("Ctrl+D")?;
            ensure!(
                terminal.wait_for_exit()? == 0,
                "custom-bind Ctrl+D did not exit successfully"
            );
            Ok(())
        },
    )?;
    let metadata = fs::symlink_metadata(&bind_path)?;
    ensure!(
        metadata.file_type().is_socket(),
        "replacement socket was removed during IPC cleanup"
    );
    drop(replacement_listener);
    fs::remove_file(bind_path)?;
    Ok(())
}

#[test]
fn ipc_pid_file_is_created_and_removed_after_q_exit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let bind_path = custom_bind_path(temp.path());
    let pid_path = temp.path().join("arf.pid");
    let mut metadata_path = None;
    run_case_with(
        Terminal::builder("ipc-pid-file-q").args([
            "--with-ipc",
            "--ipc-bind",
            bind_path.as_str(),
            "--ipc-pid-file",
            pid_path.to_string_lossy().as_ref(),
        ]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            metadata_path = Some(assert_pid_file(terminal, &pid_path)?);
            terminal.enter("q('no')")?;
            ensure!(
                terminal.wait_for_exit()? == 0,
                "q() did not exit successfully"
            );
            Ok(())
        },
    )?;
    wait_for_path_absent(&pid_path)?;
    wait_for_path_absent(
        metadata_path
            .as_deref()
            .context("q() metadata path was not captured")?,
    )?;
    #[cfg(unix)]
    wait_for_path_absent(Path::new(&bind_path))?;
    Ok(())
}

#[test]
fn ipc_pid_file_is_removed_after_ctrl_d_exit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let pid_path = temp.path().join("arf.pid");
    let mut metadata_path = None;
    run_case_with(
        Terminal::builder("ipc-pid-file-ctrl-d").args([
            "--with-ipc",
            "--ipc-pid-file",
            pid_path.to_string_lossy().as_ref(),
        ]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            metadata_path = Some(assert_pid_file(terminal, &pid_path)?);
            terminal.key("Ctrl+D")?;
            ensure!(
                terminal.wait_for_exit()? == 0,
                "Ctrl+D did not exit successfully"
            );
            Ok(())
        },
    )?;
    wait_for_path_absent(&pid_path)?;
    wait_for_path_absent(
        metadata_path
            .as_deref()
            .context("Ctrl+D metadata path was not captured")?,
    )?;
    Ok(())
}

#[test]
fn existing_ipc_pid_file_is_rejected_without_mutation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let bind_path = custom_bind_path(temp.path());
    let pid_path = temp.path().join("arf.pid");
    let sentinel = "existing-pid-file-sentinel";
    fs::write(&pid_path, sentinel)?;
    let mut metadata_path = None;
    run_case_with(
        Terminal::builder("ipc-pid-file-existing").args([
            "--with-ipc",
            "--ipc-bind",
            bind_path.as_str(),
            "--ipc-pid-file",
            pid_path.to_string_lossy().as_ref(),
        ]),
        |terminal| {
            let pid = terminal.pid().context("missing arf PID")?;
            metadata_path = Some(session_metadata_path(terminal)?);
            let exit_code = terminal.wait_for_exit()?;
            ensure!(
                exit_code != 0,
                "startup unexpectedly succeeded with an existing PID file (pid {pid})"
            );
            ensure!(
                fs::read_to_string(&pid_path)? == sentinel,
                "existing PID file was modified"
            );
            Ok(())
        },
    )?;
    ensure!(
        fs::read_to_string(&pid_path)? == sentinel,
        "existing PID file was modified after process exit"
    );
    wait_for_path_absent(
        metadata_path
            .as_deref()
            .context("rejected-startup metadata path was not captured")?,
    )?;
    #[cfg(unix)]
    wait_for_path_absent(Path::new(&bind_path))?;
    Ok(())
}

#[test]
fn condition_observer_follows_ipc_without_extra_repl_tasks() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let profile = temp.path().join("profile.R");
    fs::write(
        &profile,
        r#"
.counter <- 0L
base::globalCallingHandlers(error = function(e) base::invisible(NULL))
base::addTaskCallback(function(expr, value, ok, visible) {
    .counter <<- .counter + 1L
    TRUE
}, name = "user-profile")
"#,
    )?;
    for initially_enabled in [false, true] {
        let mut builder = Terminal::builder(format!("ipc-observer-{initially_enabled}"))
            .vanilla(false)
            .rows(60)
            .args([
                "--no-environ",
                "--no-site-file",
                "--no-save",
                "--no-restore",
            ])
            .env("R_PROFILE_USER", profile.to_string_lossy());
        if initially_enabled {
            builder = builder.args(["--with-ipc"]);
        }
        run_case_with(builder, |terminal| {
            terminal.wait_for_first_prompt()?;
            let (handlers, tasks) = if initially_enabled { (3, 2) } else { (1, 1) };
            terminal.submit(
                r#"cat("OBSERVER_BOOT:", .counter, length(globalCallingHandlers()), length(getTaskCallbackNames()), "\nOBSERVER_BOOT_DONE\n")"#,
                "OBSERVER_BOOT_DONE", PROMPT,
            )?;
            ensure!(
                terminal
                    .state()?
                    .text
                    .lines()
                    .any(|line| line == format!("OBSERVER_BOOT: 0 {handlers} {tasks}")),
                "observer startup changed R state: {:?}",
                terminal.state()?.text
            );
            for (index, enabled) in [!initially_enabled, initially_enabled]
                .into_iter()
                .enumerate()
            {
                // A real user expression sets both the expected callback count
                // and .Last.value immediately before the meta command.
                terminal.submit(
                    r#"{ .expected_count <- .counter + 1L; 54321L }"#,
                    "[1] 54321",
                    PROMPT,
                )?;
                let checkpoint = terminal.checkpoint()?;
                terminal.enter(if enabled { ":ipc start" } else { ":ipc stop" })?;
                terminal.key("enter")?;
                let message = if enabled {
                    "IPC server started:"
                } else {
                    "IPC server stopped."
                };
                terminal.wait_for("IPC transition and prompt", |_, line| {
                    line.trim_end() == PROMPT
                        && terminal
                            .output_since(checkpoint)
                            .is_ok_and(|output| output.contains(message))
                })?;
                let (handlers, tasks) = if enabled { (3, 2) } else { (1, 1) };
                let code = format!(
                    r#"cat("OBSERVER_{index}:", .counter == .expected_count, identical(.Last.value, 54321L), length(globalCallingHandlers()), length(getTaskCallbackNames()), "\nOBSERVER_DONE_{index}\n")"#
                );
                terminal.submit(&code, &format!("OBSERVER_DONE_{index}"), PROMPT)?;
                ensure!(
                    terminal
                        .state()?
                        .text
                        .lines()
                        .any(|line| line
                            == format!("OBSERVER_{index}: TRUE TRUE {handlers} {tasks}")),
                    "IPC transition changed R state: {:?}",
                    terminal.state()?.text
                );
            }
            terminal.key("Ctrl+D")?;
            ensure!(
                terminal.wait_for_exit()? == 0,
                "observer test did not exit cleanly"
            );
            Ok(())
        })?;
    }
    Ok(())
}
