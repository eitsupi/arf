//! Regression tests for interactive process behavior.
//!
//! TODO: Two paths through `:switch` are missing here because reaching them
//! needs rig and a second R version, which CI does not install: a `:restart`
//! before a `:switch`, and the same again with `LD_LIBRARY_PATH` first emptied
//! of the R library directory so the restart re-execs a second time. Both once
//! carried the previous version's library paths into the new session, and both
//! are verified by hand today. They belong in a job that installs rig and two R
//! versions, kept out of the ordinary test run.

mod common;

#[cfg(unix)]
use common::Terminal;

/// Test that restarting the same R version preserves session environment changes.
#[test]
#[cfg(unix)]
fn test_pty_restart_preserves_session_environment() {
    // This test waits for the restarted session's startup banner, so it needs
    // an isolated config that forces `show_banner = true`: `spawn_with_args`
    // does not isolate the process from the user's real config file, and if
    // the user has `startup.show_banner = false` there, the banner never
    // appears and the wait below times out even though the restart itself
    // worked correctly.
    let config_dir = tempfile::tempdir().expect("Should create a temp config dir");
    let config_path = config_dir.path().join("arf.toml");
    std::fs::write(&config_path, "[startup]\nshow_banner = true\n")
        .expect("Should write the temp config file");

    let mut terminal = Terminal::spawn_with_args(&[
        "--no-auto-match",
        "--no-completion",
        "--config",
        config_path
            .to_str()
            .expect("Config path should be valid UTF-8"),
    ])
    .expect("Failed to spawn arf");

    terminal.wait_for_prompt().expect("Should show prompt");

    // Read the value back in the same line: waiting for a bare prompt would
    // match the one reedline repaints while echoing the input.
    terminal
        .clear_buffer()
        .expect("Should clear output before setting the sentinel");
    terminal
        .send_line(r#"Sys.setenv(R_LIBS = "arf_restart_test_sentinel"); Sys.getenv("R_LIBS")"#)
        .expect("Should set the environment sentinel");
    terminal
        .expect(r#"[1] "arf_restart_test_sentinel""#)
        .expect("The sentinel should be set before restarting");

    terminal
        .clear_buffer()
        .expect("Should clear output before restart");
    terminal
        .send_line(":restart!")
        .expect("Should send :restart!");
    terminal
        .expect("Restarting R session...")
        .expect("Should announce the restart");
    // Wait for the restarted session's banner rather than a prompt, and do not
    // clear in between: the buffer was cleared before `:restart!`, so this can
    // only be the banner the new session prints. Sending the query earlier
    // risks the outgoing session answering it, which would pass even if the
    // restart had dropped the variable.
    terminal
        .expect("is ready.")
        .expect("Should wait for the restarted session");

    terminal
        .clear_buffer()
        .expect("Should clear output before reading the sentinel");
    terminal
        .send_line(r#"Sys.getenv("R_LIBS")"#)
        .expect("Should read the environment sentinel after restart");
    terminal
        .expect(r#"[1] "arf_restart_test_sentinel""#)
        .expect("The session environment should survive restart");

    terminal.quit().expect("Should quit cleanly");
}

/// A Unix exec restart must adopt the existing PID file instead of briefly
/// deleting it or failing create_new. The PID stays stable, while the new
/// image re-registers atexit cleanup for the same file.
#[test]
#[cfg(unix)]
fn test_pty_restart_preserves_pid_file_ownership() {
    use std::os::unix::net::UnixStream;

    let initial_dir = std::env::current_dir().expect("Should read the test working directory");
    let config_dir = tempfile::tempdir_in(&initial_dir).expect("Should create a temp config dir");
    let changed_dir =
        tempfile::tempdir_in(&initial_dir).expect("Should create a changed working directory");
    let config_path = config_dir.path().join("arf.toml");
    std::fs::write(&config_path, "[startup]\nshow_banner = true\n")
        .expect("Should write the temp config file");
    let pid_path = config_dir.path().join("arf.pid");
    let relative_pid_path = relative_path_from(&initial_dir, &pid_path);
    let path_from_changed_dir = changed_dir.path().join(&relative_pid_path);
    assert_ne!(
        path_from_changed_dir, pid_path,
        "The relative PID argument must resolve differently after setwd"
    );
    // Keep the socket under the test's initial directory. macOS limits the
    // length of Unix socket addresses, and the system temp directory can have
    // a much longer runner-specific prefix than this workspace path.
    let socket_dir =
        tempfile::tempdir_in(&initial_dir).expect("Should create a temp socket directory");
    let socket_path = socket_dir.path().join("custom.sock");
    let _ = std::fs::remove_file(&socket_path);
    let relative_socket_path = relative_path_from(&initial_dir, &socket_path);
    let socket_path_from_changed_dir = changed_dir.path().join(&relative_socket_path);
    assert_ne!(
        socket_path_from_changed_dir, socket_path,
        "The relative IPC bind argument must resolve differently after setwd"
    );
    let socket_arg = relative_socket_path
        .to_str()
        .expect("Relative socket path should be valid UTF-8");
    let config_arg = config_path
        .to_str()
        .expect("Config path should be valid UTF-8");

    let mut terminal = Terminal::spawn_with_size_and_env_in(
        &[
            "--with-ipc",
            "--ipc-pid-file",
            relative_pid_path
                .to_str()
                .expect("Relative PID path should be valid UTF-8"),
            "--ipc-bind",
            socket_arg,
            "--no-auto-match",
            "--no-completion",
            "--config",
            config_arg,
        ],
        &[],
        24,
        80,
        Some(&initial_dir),
    )
    .expect("Failed to spawn arf with IPC");
    terminal.wait_for_prompt().expect("Should show prompt");

    let pid_before = terminal.process_id().expect("Should have process ID");
    assert_eq!(
        std::fs::read_to_string(&pid_path).expect("PID file should be readable"),
        pid_before.to_string()
    );
    assert!(
        socket_path.exists(),
        "Custom socket should exist before restart"
    );

    terminal
        .clear_buffer()
        .expect("Should clear output before changing the working directory");
    let changed_dir_arg = changed_dir
        .path()
        .to_str()
        .expect("Changed directory should be valid UTF-8")
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    terminal
        .send_line(&format!("setwd(\"{changed_dir_arg}\"); getwd()"))
        .expect("Should change the R working directory before restart");
    let changed_dir_text = changed_dir
        .path()
        .to_str()
        .expect("Changed directory should be valid UTF-8");
    terminal
        .expect(&format!("[1] \"{changed_dir_text}\""))
        .expect("R should report the changed working directory");

    terminal
        .clear_buffer()
        .expect("Should clear output before restart");
    terminal
        .send_line(":restart!")
        .expect("Should send :restart!");
    terminal
        .expect("Restarting R session...")
        .expect("Should announce the restart");
    terminal
        .expect("is ready.")
        .expect("Should wait for the replacement image");

    assert!(pid_path.exists(), "PID file must remain during restart");
    assert_eq!(
        std::fs::read_to_string(&pid_path).expect("PID file should remain readable"),
        pid_before.to_string()
    );
    UnixStream::connect(&socket_path)
        .expect("Custom socket should accept connections after restart");
    terminal.quit().expect("Should quit cleanly");
    assert!(!pid_path.exists(), "PID file should be cleaned up on exit");
}

#[cfg(unix)]
fn relative_path_from(base: &std::path::Path, target: &std::path::Path) -> std::path::PathBuf {
    let base_components: Vec<_> = base.components().collect();
    let target_components: Vec<_> = target.components().collect();
    let common = base_components
        .iter()
        .zip(&target_components)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = std::path::PathBuf::new();
    for _ in common..base_components.len() {
        relative.push("..");
    }
    for component in target_components.iter().skip(common) {
        relative.push(component.as_os_str());
    }
    relative
}
