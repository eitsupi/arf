//! Bounded, read-only probing of the selected R executable.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const OUTPUT_LIMIT: usize = 64 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProbeError {
    Unavailable,
    Timeout,
    OutputLimit,
    Failed,
    NoHome,
}

impl ProbeError {
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::Unavailable => "the R executable could not be probed",
            Self::Timeout => "R RHOME exceeded its time limit",
            Self::OutputLimit => "R RHOME exceeded its output limit",
            Self::Failed => "R RHOME failed",
            Self::NoHome => "R RHOME returned no usable path",
        }
    }
}

pub(super) fn r_home(executable: &Path) -> Result<PathBuf, ProbeError> {
    // Source resolution runs synchronously, before the IPC runtime starts.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| ProbeError::Unavailable)?;
    let mut command = Command::new(executable);
    command.arg("RHOME");
    runtime.block_on(run(command, PROBE_TIMEOUT))
}

async fn capture(mut pipe: impl AsyncRead + Unpin) -> Result<Vec<u8>, ProbeError> {
    let mut output = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let count = pipe
            .read(&mut buffer)
            .await
            .map_err(|_| ProbeError::Unavailable)?;
        if count == 0 {
            return Ok(output);
        }
        if output.len() + count > OUTPUT_LIMIT {
            return Err(ProbeError::OutputLimit);
        }
        output.extend_from_slice(&buffer[..count]);
    }
}

async fn run(mut command: Command, deadline: Duration) -> Result<PathBuf, ProbeError> {
    command
        .env_remove("R_HOME")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|_| ProbeError::Unavailable)?;
    let stdout = child.stdout.take().ok_or(ProbeError::Unavailable)?;
    let stderr = child.stderr.take().ok_or(ProbeError::Unavailable)?;
    // Include pipe EOF in the deadline: launchers can exit while a descendant
    // still holds a pipe open. Cancellation drops the pipes without a reader join.
    let result = tokio::time::timeout(deadline, async {
        tokio::try_join!(capture(stdout), capture(stderr), async {
            child.wait().await.map_err(|_| ProbeError::Unavailable)
        })
    })
    .await
    .map_err(|_| ProbeError::Timeout)
    .and_then(|result| result);
    match result {
        Ok((stdout, _, status)) => {
            if !status.success() {
                return Err(ProbeError::Failed);
            }
            let output = std::str::from_utf8(&stdout).map_err(|_| ProbeError::NoHome)?;
            arf_libr::r_home_from_rhome_output(output)
                .map(PathBuf::from)
                .ok_or(ProbeError::NoHome)
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
                        "app::setup::pixi::probe::tests::probe_child",
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
    async fn process_and_output_limits_are_bounded() {
        for (mode, expected) in [
            ("home", Ok(PathBuf::from("/test/R/home"))),
            ("invalid_utf8", Err(ProbeError::NoHome)),
            ("fail", Err(ProbeError::Failed)),
            ("stdout", Err(ProbeError::OutputLimit)),
            ("stderr", Err(ProbeError::OutputLimit)),
            ("hang", Err(ProbeError::Timeout)),
            ("descendant", Err(ProbeError::Timeout)),
            ("exited_parent", Err(ProbeError::Timeout)),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let pid_file = temp.path().join("descendant.pid");
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "app::setup::pixi::probe::tests::probe_child",
                    "--nocapture",
                    "--quiet",
                ])
                .env("ARF_TEST_RHOME_PROBE", mode)
                .env("ARF_TEST_PROBE_PID_FILE", &pid_file)
                .env("R_HOME", "/wrong/inherited/home");
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
