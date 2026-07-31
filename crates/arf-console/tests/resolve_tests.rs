#[cfg(not(windows))]
use std::path::PathBuf;
use std::process::Command;
#[cfg(not(windows))]
use std::process::Stdio;
#[cfg(not(windows))]
use std::thread;
#[cfg(not(windows))]
use std::time::{Duration, Instant};

#[cfg(not(windows))]
struct RLessEnvironment {
    _temp: tempfile::TempDir,
    bin_dir: PathBuf,
    fake_r_home: PathBuf,
}

#[cfg(not(windows))]
impl RLessEnvironment {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let bin_dir = temp.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap();
        let fake_r_home = {
            let fake_r_home = temp.path().join("fake-r-home");
            let fake_r_library = arf_libr::r_library_path(&fake_r_home);
            std::fs::create_dir_all(fake_r_library.parent().unwrap()).unwrap();
            std::fs::write(fake_r_library, []).unwrap();
            fake_r_home
        };

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
        #[cfg(unix)]
        let path_entries = vec![self.bin_dir.clone()];
        #[cfg(not(unix))]
        let path_entries = vec![self.bin_dir.clone()];
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
fn resolve_found_emits_descriptor_with_target() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r", "resolve", "--r-home"])
        .arg(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["resolved"], true);
    assert_eq!(value["target"]["r_home"], temp.path().display().to_string());
    assert!(value["target"]["r_binary"].is_null());
    assert!(value["target"]["resolved_version"].is_null());
    assert_eq!(value["selected_by"]["kind"], "explicit_r_home");
    assert_eq!(value["selected_by"]["origin"], "cli");
    assert!(value["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn resolve_not_found_is_successful_false_descriptor() {
    let environment = RLessEnvironment::new();
    let output = environment
        .command()
        .args(["r", "resolve", "--no-r-source-overrides"])
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    if value["resolved"] != false {
        eprintln!("skipping unresolved discovery assertion because a built-in R was found");
        return;
    }
    assert_eq!(value["resolved"], false);
    assert!(value["target"].is_null());
    assert!(value["diagnostics"].is_array());
}

#[test]
fn resolve_broken_config_falls_back_with_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("broken.toml");
    std::fs::write(&config, "not valid = [toml").unwrap();
    let r_home = temp.path().join("r-home");
    std::fs::create_dir(&r_home).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r", "resolve", "--config"])
        .arg(&config)
        .args(["--r-home"])
        .arg(&r_home)
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["resolved"], true);
    assert_eq!(value["target"]["r_home"], r_home.display().to_string());
    assert!(
        value["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "config.parse_failed")
    );
}

#[test]
fn resolve_environment_source_reports_environment_origin() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r", "resolve"])
        .env("ARF_R_HOME", temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["selected_by"]["kind"], "explicit_r_home");
    assert_eq!(value["selected_by"]["origin"], "environment");
}

#[test]
fn resolve_invalid_version_is_invalid_invocation() {
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r", "resolve", "--with-r-version", "not-a-version"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
#[cfg(unix)]
fn resolve_valid_but_uninstalled_version_is_invalid_invocation() {
    let temp = tempfile::tempdir().unwrap();
    let bin_dir = temp.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    let rig = bin_dir.join("rig");
    std::fs::write(
        &rig,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then exit 0; fi\nif [ \"$1\" = \"list\" ]; then printf '%s\\n' '[{\"name\":\"4.4.2\",\"default\":true,\"version\":\"4.4.2\",\"aliases\":[],\"path\":\"/opt/R/4.4.2\",\"binary\":\"/opt/R/4.4.2/bin/R\"}]'; exit 0; fi\nexit 1\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(&rig).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&rig, permissions).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r", "resolve", "--with-r-version", "4.9.9"])
        .env("PATH", &bin_dir)
        .env_remove("ARF_R_HOME")
        .env_remove("ARF_R_VERSION")
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("4.9.9")
    );
}

fn assert_structured_resolve_error(output: &std::process::Output, code: &str) {
    assert!(output.stdout.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    let error = value["error"].as_object().unwrap();
    assert_eq!(error["code"], code);
    assert!(error["message"].is_string());
    assert!(error["hint"].is_null());
    assert!(error["data"].is_null());
}

/// This covers initialization-failure fallback, not PATH discovery.
#[test]
#[cfg(not(windows))]
fn startup_survives_without_r_on_path() {
    let environment = RLessEnvironment::new();
    let mut child = environment
        .startup_command()
        .args(["--no-banner", "--no-history"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let timeout = Duration::from_secs(10);
    let start = Instant::now();
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "arf did not exit within {timeout:?}; stderr={}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(50));
    }

    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "arf should start and exit cleanly without a usable R library: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("R evaluation will not be available"));
}
