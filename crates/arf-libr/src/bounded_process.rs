//! Bounded capture for subprocesses used during R discovery.

use std::process::{Command, Output, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command as AsyncCommand;

/// Maximum bytes captured from each subprocess output stream.
pub const BOUNDED_COMMAND_OUTPUT_LIMIT: usize = 64 * 1024;
/// Maximum time allowed for a subprocess and both output pipes to finish.
pub const BOUNDED_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BoundedProcessError {
    #[error("the subprocess could not be run")]
    Unavailable,
    #[error("the subprocess exceeded its time limit")]
    Timeout,
    #[error("the subprocess exceeded its output limit")]
    OutputLimit,
}

/// Run a configured command with bounded output and execution time.
///
/// The command's arguments, environment, and working directory are preserved.
/// Standard input is closed, stdout and stderr are captured independently up
/// to [`BOUNDED_COMMAND_OUTPUT_LIMIT`] each, and process completion plus pipe
/// EOF must occur within [`BOUNDED_COMMAND_TIMEOUT`]. Direct-child cleanup has
/// a separate one-second deadline. A dedicated thread owns the runtime so this
/// synchronous API can also be called from an existing async runtime.
///
/// The returned [`Output`] retains stdout and stderr as raw bytes and keeps
/// the child's exit status unchanged, including for nonzero exits. Callers
/// interpret the status and bytes according to the command they ran.
pub fn run_bounded_command(command: Command) -> Result<Output, BoundedProcessError> {
    std::thread::Builder::new()
        .name("bounded subprocess".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| BoundedProcessError::Unavailable)?;
            runtime.block_on(run(AsyncCommand::from(command), BOUNDED_COMMAND_TIMEOUT))
        })
        .map_err(|_| BoundedProcessError::Unavailable)?
        .join()
        .map_err(|_| BoundedProcessError::Unavailable)?
}

async fn capture(mut pipe: impl AsyncRead + Unpin) -> Result<Vec<u8>, BoundedProcessError> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = pipe
            .read(&mut buffer)
            .await
            .map_err(|_| BoundedProcessError::Unavailable)?;
        if count == 0 {
            return Ok(output);
        }
        if output.len() + count > BOUNDED_COMMAND_OUTPUT_LIMIT {
            return Err(BoundedProcessError::OutputLimit);
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

async fn run(mut command: AsyncCommand, deadline: Duration) -> Result<Output, BoundedProcessError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .map_err(|_| BoundedProcessError::Unavailable)?;
    let stdout = child
        .stdout
        .take()
        .ok_or(BoundedProcessError::Unavailable)?;
    let stderr = child
        .stderr
        .take()
        .ok_or(BoundedProcessError::Unavailable)?;
    // Include pipe EOF in the deadline: launchers can exit while a descendant
    // still holds a pipe open. Cancellation drops the pipes without a reader join.
    let result = tokio::time::timeout(deadline, async {
        tokio::try_join!(capture(stdout), capture(stderr), async {
            child
                .wait()
                .await
                .map_err(|_| BoundedProcessError::Unavailable)
        })
    })
    .await
    .map_err(|_| BoundedProcessError::Timeout)
    .and_then(|result| result);
    match result {
        Ok((stdout, stderr, status)) => Ok(Output {
            status,
            stdout,
            stderr,
        }),
        Err(error) => {
            // Bound direct-child cleanup too. We do not promise to kill all
            // descendants of arbitrary launchers; no blocking reader survives.
            let _ = child.start_kill();
            let _ = tokio::time::timeout(CLEANUP_TIMEOUT, child.wait()).await;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::io::Write;
    use std::path::PathBuf;

    // Use this test executable as a portable child fixture without requiring
    // R, Pixi, Python, or a shell on the CI runner.
    #[test]
    fn process_child() {
        let Ok(mode) = std::env::var("ARF_TEST_BOUNDED_PROCESS") else {
            return;
        };
        match mode.as_str() {
            "raw" => {
                let args: Vec<_> = std::env::args_os().collect();
                assert!(args[1..].iter().map(OsStr::new).eq([
                    OsStr::new("--exact"),
                    OsStr::new("bounded_process::tests::process_child"),
                    OsStr::new("--nocapture"),
                    OsStr::new("--quiet"),
                ]));
                assert_eq!(std::env::var("ARF_TEST_EXPECTED_ENV").unwrap(), "preserved");
                assert_eq!(
                    std::env::current_dir().unwrap().canonicalize().unwrap(),
                    PathBuf::from(std::env::var_os("ARF_TEST_EXPECTED_CWD").unwrap())
                        .canonicalize()
                        .unwrap()
                );
                std::io::stdout().write_all(&[0, 0xff, b'o', b'k']).unwrap();
                std::io::stderr()
                    .write_all(&[0xfe, b'e', b'r', b'r'])
                    .unwrap();
                std::process::exit(7);
            }
            "rhome" => {
                assert!(std::env::var_os("R_HOME").is_none());
                println!("WARNING: ignored\n\n/test/R/home\n");
            }
            "inherited_rhome" => {
                assert_eq!(std::env::var("R_HOME").unwrap(), "/caller/R/home");
                println!("/caller/R/home");
            }
            "invalid_utf8" => {
                std::io::stdout().write_all(&[0xff]).unwrap();
            }
            "fail" => std::process::exit(7),
            "stdout" | "stderr" => {
                let bytes = vec![b'x'; BOUNDED_COMMAND_OUTPUT_LIMIT + 4096];
                if mode == "stdout" {
                    let _ = std::io::stdout().write_all(&bytes);
                } else {
                    let _ = std::io::stderr().write_all(&bytes);
                }
                std::thread::sleep(Duration::from_secs(10));
            }
            "hang" => std::thread::sleep(Duration::from_secs(10)),
            "descendant" | "exited_parent" => {
                #[expect(
                    clippy::zombie_processes,
                    reason = "This fixture deliberately exits without waiting; the controller kills its descendant PID"
                )]
                let child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "bounded_process::tests::process_child",
                        "--nocapture",
                        "--quiet",
                    ])
                    .env("ARF_TEST_BOUNDED_PROCESS", "hang")
                    .spawn()
                    .unwrap();
                std::fs::write(
                    std::env::var_os("ARF_TEST_PROBE_PID_FILE").unwrap(),
                    child.id().to_string(),
                )
                .unwrap();
                if mode == "descendant" {
                    std::thread::sleep(Duration::from_secs(10));
                }
            }
            _ => panic!("unknown bounded process fixture"),
        }
        // Avoid libtest's own output after the simulated subprocess output.
        std::process::exit(0);
    }

    fn fixture_command(mode: &str) -> AsyncCommand {
        let mut command = AsyncCommand::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "bounded_process::tests::process_child",
                "--nocapture",
                "--quiet",
            ])
            .env("ARF_TEST_BOUNDED_PROCESS", mode);
        command
    }

    #[test]
    fn captures_raw_streams_status_and_preserves_command_context() {
        let cwd = tempfile::tempdir().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "bounded_process::tests::process_child",
                "--nocapture",
                "--quiet",
            ])
            .env("ARF_TEST_BOUNDED_PROCESS", "raw")
            .env("ARF_TEST_EXPECTED_ENV", "preserved")
            .env("ARF_TEST_EXPECTED_CWD", cwd.path())
            .env("R_HOME", "/parent/R/home")
            .current_dir(cwd.path());
        let output = run_bounded_command(command).unwrap();
        assert!(output.stdout.ends_with(&[0, 0xff, b'o', b'k']));
        assert_eq!(output.stderr, [0xfe, b'e', b'r', b'r']);
        assert_eq!(output.status.code(), Some(7));
    }

    #[tokio::test]
    async fn process_and_pipe_eof_limits_are_bounded() {
        for (mode, expected) in [
            ("stdout", Err(BoundedProcessError::OutputLimit)),
            ("stderr", Err(BoundedProcessError::OutputLimit)),
            ("hang", Err(BoundedProcessError::Timeout)),
            ("descendant", Err(BoundedProcessError::Timeout)),
            ("exited_parent", Err(BoundedProcessError::Timeout)),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let pid_file = temp.path().join("descendant.pid");
            let mut command = fixture_command(mode);
            command.env("ARF_TEST_PROBE_PID_FILE", &pid_file);
            let start = std::time::Instant::now();
            let result = run(command, Duration::from_secs(2)).await;
            if let Ok(pid) = std::fs::read_to_string(&pid_file) {
                let pid: u32 = pid.parse().unwrap();
                #[cfg(unix)]
                // SAFETY: This PID belongs to our isolated fixture descendant.
                unsafe {
                    libc::kill(pid as i32, libc::SIGKILL);
                }
                #[cfg(windows)]
                {
                    use windows_sys::Win32::Foundation::CloseHandle;
                    use windows_sys::Win32::System::Threading::{
                        OpenProcess, PROCESS_TERMINATE, TerminateProcess,
                    };
                    // SAFETY: Open only our fixture PID and close the resulting handle.
                    unsafe {
                        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
                        if !handle.is_null() {
                            TerminateProcess(handle, 1);
                            CloseHandle(handle);
                        }
                    }
                }
            }
            assert_eq!(result, expected, "mode: {mode}");
            assert!(start.elapsed() < Duration::from_secs(5), "mode: {mode}");
        }
    }
}
