//! Bounded RHOME probing shared by source providers and automatic discovery.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command as AsyncCommand;

const OUTPUT_LIMIT: usize = 64 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{}", self.description())]
pub enum RHomeProbeError {
    Unavailable,
    Timeout,
    OutputLimit,
    Failed,
    NoHome,
}

impl RHomeProbeError {
    pub fn description(self) -> &'static str {
        match self {
            Self::Unavailable => "the R executable could not be probed",
            Self::Timeout => "R RHOME exceeded its time limit",
            Self::OutputLimit => "R RHOME exceeded its output limit",
            Self::Failed => "R RHOME failed",
            Self::NoHome => "R RHOME returned no usable path",
        }
    }
}

/// Query a caller-configured executable with `RHOME` under fixed limits.
///
/// The command's environment is preserved, allowing callers to choose whether
/// to clear inherited R_HOME. Standard input is closed, each output pipe is
/// capped at 64 KiB, and process completion plus pipe EOF must occur within ten
/// seconds. Direct-child cleanup has a separate one-second deadline.
///
/// A dedicated thread owns the runtime so synchronous discovery can also be
/// called from an existing async runtime without nesting Tokio runtimes.
pub fn probe_r_home(command: Command) -> Result<PathBuf, RHomeProbeError> {
    std::thread::Builder::new()
        .name("RHOME probe".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| RHomeProbeError::Unavailable)?;
            let mut command = AsyncCommand::from(command);
            command.arg("RHOME");
            runtime.block_on(run(command, PROBE_TIMEOUT))
        })
        .map_err(|_| RHomeProbeError::Unavailable)?
        .join()
        .map_err(|_| RHomeProbeError::Unavailable)?
}

async fn capture(mut pipe: impl AsyncRead + Unpin) -> Result<Vec<u8>, RHomeProbeError> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = pipe
            .read(&mut buffer)
            .await
            .map_err(|_| RHomeProbeError::Unavailable)?;
        if count == 0 {
            return Ok(output);
        }
        if output.len() + count > OUTPUT_LIMIT {
            return Err(RHomeProbeError::OutputLimit);
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

async fn run(mut command: AsyncCommand, deadline: Duration) -> Result<PathBuf, RHomeProbeError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| RHomeProbeError::Unavailable)?;
    let stdout = child.stdout.take().ok_or(RHomeProbeError::Unavailable)?;
    let stderr = child.stderr.take().ok_or(RHomeProbeError::Unavailable)?;
    // Include pipe EOF in the deadline: launchers can exit while a descendant
    // still holds a pipe open. Cancellation drops the pipes without a reader join.
    let result = tokio::time::timeout(deadline, async {
        tokio::try_join!(capture(stdout), capture(stderr), async {
            child.wait().await.map_err(|_| RHomeProbeError::Unavailable)
        })
    })
    .await
    .map_err(|_| RHomeProbeError::Timeout)
    .and_then(|result| result);
    match result {
        Ok((stdout, _, status)) => {
            if !status.success() {
                return Err(RHomeProbeError::Failed);
            }
            let output = std::str::from_utf8(&stdout).map_err(|_| RHomeProbeError::NoHome)?;
            crate::r_home_from_rhome_output(output)
                .map(PathBuf::from)
                .ok_or(RHomeProbeError::NoHome)
        }
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

    // Use this test executable as a portable child fixture, without requiring
    // R, Pixi, Python, or a shell on the CI runner.
    #[test]
    fn probe_child() {
        let Ok(mode) = std::env::var("ARF_TEST_RHOME_PROBE") else {
            return;
        };
        use std::io::Write;
        match mode.as_str() {
            "home" => {
                assert!(std::env::var_os("R_HOME").is_none());
                println!("WARNING: ignored\n\n/test/R/home\n");
            }
            "inherited_home" => {
                assert_eq!(std::env::var("R_HOME").unwrap(), "/caller/R/home");
                println!("/caller/R/home");
            }
            "invalid_utf8" => {
                std::io::stdout().write_all(&[0xff]).unwrap();
            }
            "fail" => std::process::exit(7),
            "stdout" | "stderr" => {
                let bytes = vec![b'x'; OUTPUT_LIMIT + 4096];
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
                        "rhome_probe::tests::probe_child",
                        "--nocapture",
                        "--quiet",
                    ])
                    .env("ARF_TEST_RHOME_PROBE", "hang")
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
            _ => panic!("unknown probe fixture"),
        }
        // Avoid libtest's own stdout after the simulated R output.
        std::process::exit(0);
    }

    #[tokio::test]
    async fn synchronous_probe_works_inside_an_async_runtime() {
        for (mode, expected) in [
            ("home", "/test/R/home"),
            ("inherited_home", "/caller/R/home"),
        ] {
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "rhome_probe::tests::probe_child",
                    "--nocapture",
                    "--quiet",
                ])
                .env("ARF_TEST_RHOME_PROBE", mode);
            if mode == "home" {
                command.env_remove("R_HOME");
            } else {
                command.env("R_HOME", expected);
            }
            assert_eq!(probe_r_home(command), Ok(PathBuf::from(expected)));
        }
    }

    #[tokio::test]
    async fn process_and_output_limits_are_bounded() {
        for (mode, expected) in [
            ("home", Ok(PathBuf::from("/test/R/home"))),
            ("invalid_utf8", Err(RHomeProbeError::NoHome)),
            ("fail", Err(RHomeProbeError::Failed)),
            ("stdout", Err(RHomeProbeError::OutputLimit)),
            ("stderr", Err(RHomeProbeError::OutputLimit)),
            ("hang", Err(RHomeProbeError::Timeout)),
            ("descendant", Err(RHomeProbeError::Timeout)),
            ("exited_parent", Err(RHomeProbeError::Timeout)),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let pid_file = temp.path().join("descendant.pid");
            let mut command = AsyncCommand::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "rhome_probe::tests::probe_child",
                    "--nocapture",
                    "--quiet",
                ])
                .env("ARF_TEST_RHOME_PROBE", mode)
                .env("ARF_TEST_PROBE_PID_FILE", &pid_file)
                .env_remove("R_HOME");
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
                    // SAFETY: Open only our fixture PID; close the resulting handle.
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
