//! Probe the selected Pixi launcher without inheriting an unrelated R_HOME.

use std::path::{Path, PathBuf};
use std::process::Command;

pub(super) use arf_libr::RHomeProbeError as ProbeError;

pub(super) fn r_home(executable: &Path) -> Result<PathBuf, ProbeError> {
    let mut command = Command::new(executable);
    command.env_remove("R_HOME");
    arf_libr::probe_r_home(command)
}
