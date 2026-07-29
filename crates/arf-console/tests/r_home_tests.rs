use std::path::PathBuf;
use std::process::{Command, Stdio};

struct RLessEnvironment {
    _temp: tempfile::TempDir,
    bin_dir: PathBuf,
    fake_r_home: PathBuf,
}

impl RLessEnvironment {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let bin_dir = temp.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap();
        let fake_r_home = temp.path().join("fake-r-home");
        let fake_r_library = arf_libr::r_library_path(&fake_r_home);
        std::fs::create_dir_all(fake_r_library.parent().unwrap()).unwrap();
        std::fs::write(fake_r_library, []).unwrap();

        #[cfg(unix)]
        {
            let r_command = bin_dir.join("R");
            std::fs::write(&r_command, "#!/bin/sh\nexit 1\n").unwrap();
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&r_command).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&r_command, permissions).unwrap();
        }

        Self {
            _temp: temp,
            bin_dir,
            fake_r_home,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_arf"));
        let mut path_entries = vec![self.bin_dir.clone()];
        #[cfg(unix)]
        {
            path_entries.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
        }
        command
            .env_remove("R_HOME")
            .env_remove("ARF_R_HOME")
            .env_remove("ARF_R_VERSION")
            .env("PATH", std::env::join_paths(path_entries).unwrap());
        command
    }

    fn startup_command(&self) -> Command {
        let mut command = self.command();
        command.env("R_HOME", &self.fake_r_home);
        command
    }
}

#[test]
fn plain_r_home_output_is_exactly_the_resolved_path() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r-home", "--r-home"])
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{}\n", temp.path().display())
    );
    assert!(String::from_utf8(output.stderr).unwrap().is_empty());
}

#[test]
fn json_r_home_output_has_resolution_details() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r-home", "--json", "--r-home"])
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["r_home"], temp.path().display().to_string());
    assert_eq!(value["source"], format!("path ({})", temp.path().display()));
    assert_eq!(value["r_source_override"]["state"], "shadowed_by_cli");
    for field in [
        "provider",
        "file",
        "key",
        "requested_version",
        "resolved_version",
    ] {
        assert!(value["r_source_override"][field].is_null(), "{field}");
    }
    assert!(value["warnings"].as_array().unwrap().is_empty());
}

#[test]
fn r_home_resolution_failure_exits_nonzero() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing-r-home");
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r-home", "--r-home"])
        .arg(missing)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("R_HOME path does not exist")
    );
}

#[test]
fn startup_survives_without_r_on_path() {
    let environment = RLessEnvironment::new();
    let output = environment
        .startup_command()
        .args(["--no-banner", "--no-history"])
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "arf should start and exit cleanly without a usable R library: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("R evaluation will not be available"));
}

#[test]
fn r_home_without_r_on_path_exits_nonzero_with_null_json_path() {
    let environment = RLessEnvironment::new();
    let output = environment
        .command()
        .args(["r-home", "--json", "--no-r-source-overrides"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value["r_home"].is_null());
    assert!(value["warnings"].as_array().unwrap().iter().any(|warning| {
        warning
            .as_str()
            .unwrap()
            .contains("Failed to determine R_HOME from PATH")
    }));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Could not determine R_HOME from PATH")
    );
}
