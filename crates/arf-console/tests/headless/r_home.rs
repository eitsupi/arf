use super::support::*;
use std::path::{Path, PathBuf};

/// Locate a directory holding a real `R` executable but no `rig`.
///
/// Restricting `PATH` to the first directory on it that contains `R` is not
/// enough: rig installs its own `R` shim next to the `rig` binary, so that
/// directory usually holds both and rig stays available. A session that can
/// still reach rig never enters PATH mode, which is the mode under test.
///
/// Resolving the shim to its target gives the R installation's own `bin`
/// directory, which does not contain rig.
fn r_bin_dir_without_rig() -> Option<PathBuf> {
    let r_command = if cfg!(windows) { "R.exe" } else { "R" };
    let rig_command = if cfg!(windows) { "rig.exe" } else { "rig" };

    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(r_command))
        .filter(|r| r.is_file())
        .filter_map(|r| std::fs::canonicalize(r).ok())
        .filter_map(|r| r.parent().map(Path::to_path_buf))
        .find(|dir| !dir.join(rig_command).exists())
}

/// The R_HOME reported over IPC must come from the running R, not from the
/// startup resolution, which leaves it unset in PATH mode.
#[test]
fn test_headless_json_reports_runtime_r_home_in_path_mode() {
    let Some(r_bin_dir) = r_bin_dir_without_rig() else {
        panic!("this test needs a directory on PATH holding R but not rig");
    };
    let restricted_path = r_bin_dir.to_str().expect("R path should be UTF-8");

    let process =
        HeadlessProcess::spawn_with_args_and_env(&["--json"], &[("PATH", restricted_path)])
            .expect("Failed to spawn headless in PATH mode");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while process.stdout_output().trim().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "Timed out waiting for JSON on stdout. stderr: {}",
            process.stderr_output()
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let stdout = process.stdout_output();
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|error| panic!("Invalid JSON: {error}\nstdout: {stdout}"));
    let r_home = json["r_home"]
        .as_str()
        .expect("PATH mode should report a runtime r_home");
    assert!(
        Path::new(r_home).is_absolute(),
        "runtime r_home should be absolute: {r_home}"
    );
}
