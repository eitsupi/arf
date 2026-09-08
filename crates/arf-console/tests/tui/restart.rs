use super::support::{DEFAULT_CONFIG, PROMPT, Terminal, run_case_with, wait_for_path_absent};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(30);

#[cfg(unix)]
fn custom_bind_path(root: &Path) -> String {
    root.join("arf-restart.sock").display().to_string()
}

#[cfg(not(unix))]
fn custom_bind_path(root: &Path) -> String {
    let unique = root
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| "session".into());
    format!(r"\\.\pipe\arf-tui-restart-{unique}")
}

fn metadata_path(terminal: &Terminal, pid: u32) -> PathBuf {
    terminal.sessions_dir().join(format!("{pid}.json"))
}

fn read_pid(path: &Path) -> Result<u32> {
    Ok(fs::read_to_string(path)?.trim().parse()?)
}

fn wait_for_replacement(
    terminal: &Terminal,
    checkpoint: usize,
    pid_path: &Path,
    bind_path: &str,
    changed_cwd: &str,
) -> Result<(u32, PathBuf, Value)> {
    let started = Instant::now();
    loop {
        let state = terminal.state()?;
        let line = terminal.screen_line(state.cursor.y, state.cols)?;
        let output = terminal.output_since(checkpoint)?;
        if state.exited.is_none()
            && line.trim_end() == PROMPT
            && output.contains("Restarting R session...")
            && output.contains("is ready.")
            && let Ok(pid) = read_pid(pid_path)
        {
            let metadata = metadata_path(terminal, pid);
            if let Ok(contents) = fs::read_to_string(&metadata)
                && let Ok(json) = serde_json::from_str::<Value>(&contents)
                && json["socket_path"] == bind_path
                && json["cwd"] == changed_cwd
            {
                return Ok((pid, metadata, json));
            }
        }
        ensure!(
            started.elapsed() < WAIT,
            "timed out waiting for replacement readiness: {state:?}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn restart_preserves_environment_and_reconnects_ipc() -> Result<()> {
    // Keep Unix socket paths short enough for macOS sockaddr limits. The
    // system temp directory may have a long runner-specific prefix.
    let test_cwd = std::env::current_dir()?;
    let initial_dir = tempfile::tempdir_in(&test_cwd)?;
    let changed_dir = tempfile::tempdir_in(&test_cwd)?;
    let pid_path = initial_dir.path().join("arf-restart.pid");
    let bind_path = custom_bind_path(initial_dir.path());
    #[cfg(unix)]
    let bind_arg = "arf-restart.sock".to_owned();
    #[cfg(not(unix))]
    let bind_arg = bind_path.clone();
    let changed_cwd = changed_dir.path().to_string_lossy().into_owned();
    let changed_literal = serde_json::to_string(&changed_cwd)?;
    #[cfg(unix)]
    ensure!(
        changed_dir.path().join(&bind_arg) != Path::new(&bind_path),
        "setwd must change how the relative IPC bind path would resolve"
    );
    let mut initial_metadata = None;
    let mut replacement_metadata = None;

    run_case_with(
        Terminal::builder("restart-ipc")
            .cwd(initial_dir.path())
            .config("[startup]\nshow_banner = true\n".to_owned() + DEFAULT_CONFIG)
            .args([
                "--with-ipc",
                "--ipc-eval-unrestricted",
                "--ipc-pid-file",
                "arf-restart.pid",
                "--ipc-bind",
                bind_arg.as_str(),
                "--no-auto-match",
                "--no-completion",
            ]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let pid_before = terminal.pid().context("missing initial arf PID")?;
            ensure!(
                read_pid(&pid_path)? == pid_before,
                "initial PID file does not identify arf"
            );
            let metadata_before = metadata_path(terminal, pid_before);
            ensure!(
                metadata_before.is_file(),
                "initial session metadata is missing"
            );
            initial_metadata = Some(metadata_before);

            let initial_session = terminal.start_ipc(&["session"])?.finish()?;
            ensure!(
                initial_session["socket_path"] == bind_path,
                "initial IPC bind path is wrong: {initial_session}"
            );
            terminal.submit(
                r#"Sys.setenv(R_LIBS = "restart_environment_sentinel"); Sys.getenv("R_LIBS")"#,
                r#"[1] "restart_environment_sentinel""#,
                PROMPT,
            )?;
            terminal.submit(
                &format!("setwd({changed_literal}); cat('RESTART_CWD_CHANGED\\n')"),
                "RESTART_CWD_CHANGED",
                PROMPT,
            )?;

            let checkpoint = terminal.checkpoint()?;
            terminal.enter(":restart!")?;
            let (pid_after, metadata_after, metadata_json) =
                wait_for_replacement(terminal, checkpoint, &pid_path, &bind_path, &changed_cwd)?;
            replacement_metadata = Some(metadata_after);

            #[cfg(unix)]
            ensure!(
                pid_after == pid_before,
                "Unix exec restart changed PID: {pid_before} -> {pid_after}"
            );
            #[cfg(not(unix))]
            ensure!(
                pid_after != pid_before,
                "replacement child reused the initial PID: {pid_before}"
            );
            #[cfg(not(unix))]
            ensure!(
                !metadata_before.exists(),
                "old session metadata was not removed during replacement"
            );
            ensure!(
                read_pid(&pid_path)? == pid_after,
                "PID file does not identify replacement child"
            );
            ensure!(
                metadata_json["pid"] == pid_after,
                "replacement metadata has wrong PID: {metadata_json}"
            );

            let session = terminal
                .start_ipc_with_pid(&["session"], pid_after)?
                .finish()?;
            ensure!(
                session["pid"] == pid_after && session["socket_path"] == bind_path,
                "replacement IPC session is wrong: {session}"
            );
            #[cfg(unix)]
            ensure!(
                Path::new(&bind_path).exists(),
                "replacement Unix IPC socket path is missing"
            );
            let environment = terminal
                .start_ipc_with_pid(
                    &["eval", "Sys.getenv('R_LIBS')", "--timeout", "10000"],
                    pid_after,
                )?
                .finish()?;
            ensure!(
                environment["value"] == r#"[1] "restart_environment_sentinel""#,
                "environment sentinel was not preserved: {environment}"
            );
            terminal.submit("42", "[1] 42", PROMPT)?;
            terminal.quit()
        },
    )?;

    wait_for_path_absent(&pid_path)?;
    wait_for_path_absent(&initial_metadata.context("initial metadata path was not captured")?)?;
    if let Some(metadata) = replacement_metadata {
        wait_for_path_absent(&metadata)?;
    }
    Ok(())
}
