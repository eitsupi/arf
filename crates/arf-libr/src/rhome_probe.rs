//! Bounded RHOME probing shared by source providers and automatic discovery.

use std::path::PathBuf;
use std::process::Command;

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
pub fn probe_r_home(mut command: Command) -> Result<PathBuf, RHomeProbeError> {
    command.arg("RHOME");
    let output = crate::run_bounded_command(command).map_err(map_process_error)?;
    if !output.status.success() {
        return Err(RHomeProbeError::Failed);
    }
    let output = std::str::from_utf8(&output.stdout).map_err(|_| RHomeProbeError::NoHome)?;
    crate::r_home_from_rhome_output(output)
        .map(PathBuf::from)
        .ok_or(RHomeProbeError::NoHome)
}

fn map_process_error(error: crate::BoundedProcessError) -> RHomeProbeError {
    match error {
        crate::BoundedProcessError::Unavailable => RHomeProbeError::Unavailable,
        crate::BoundedProcessError::Timeout => RHomeProbeError::Timeout,
        crate::BoundedProcessError::OutputLimit => RHomeProbeError::OutputLimit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
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

    #[tokio::test]
    async fn synchronous_probe_works_inside_an_async_runtime() {
        for (mode, expected) in [
            ("rhome", "/test/R/home"),
            ("inherited_rhome", "/caller/R/home"),
        ] {
            let mut command = probe_command(mode);
            if mode == "rhome" {
                command.env_remove("R_HOME");
            } else {
                command.env("R_HOME", expected);
            }
            assert_eq!(probe_r_home(command), Ok(PathBuf::from(expected)));
        }
    }

    #[test]
    fn preserves_rhome_failure_and_invalid_utf8_meanings() {
        assert_eq!(
            probe_r_home(probe_command("invalid_utf8")),
            Err(RHomeProbeError::NoHome)
        );
        assert_eq!(
            probe_r_home(probe_command("fail")),
            Err(RHomeProbeError::Failed)
        );
    }
}
