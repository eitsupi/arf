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

#[cfg(unix)]
struct FakeRigEnvironment {
    _temp: tempfile::TempDir,
    bin_dir: PathBuf,
    fake_r_home: PathBuf,
    rig: PathBuf,
    r: PathBuf,
}

#[cfg(unix)]
impl FakeRigEnvironment {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let bin_dir = temp.path().join("bin");
        std::fs::create_dir(&bin_dir).unwrap();
        let fake_r_home = temp.path().join("fallback-r-home");
        std::fs::create_dir(&fake_r_home).unwrap();

        let r = bin_dir.join("R");
        write_executable(
            &r,
            r#"#!/bin/sh
if [ "$1" = "RHOME" ]; then
    printf '%s\n' "$FAKE_R_HOME"
    exit 0
fi
exit 1
"#,
        );

        let rig = bin_dir.join("rig");
        write_executable(
            &rig,
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then exit 0; fi
if [ "$1" = "list" ]; then printf '%s\n' '[{"name":"4.4.2","default":true,"version":"4.4.2","aliases":["my-proj"],"path":"/fake/R/4.4.2","binary":"R"} ]'; exit 0; fi
exit 1
"#,
        );

        Self {
            _temp: temp,
            bin_dir,
            fake_r_home,
            rig,
            r,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_arf"));
        command
            .env("PATH", &self.bin_dir)
            .env("FAKE_R_HOME", &self.fake_r_home)
            .env_remove("R_HOME")
            .env_remove("ARF_R_HOME")
            .env_remove("ARF_R_VERSION");
        command
    }
}

#[cfg(unix)]
fn write_executable(path: &std::path::Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).unwrap();
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
            std::fs::write(
                &r_command,
                r#"#!/bin/sh
exit 1
"#,
            )
            .unwrap();
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
    assert_eq!(
        value["selected_by"]["path"],
        temp.path().display().to_string()
    );
    assert!(value["diagnostics"].as_array().unwrap().is_empty());
}

#[test]
fn resolve_relative_r_home_uses_one_absolute_path_representation() {
    let temp = tempfile::tempdir().unwrap();
    let r_home = temp.path().join("rh");
    std::fs::create_dir(&r_home).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r", "resolve", "--r-home", "rh"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let expected = r_home.display().to_string();
    assert_eq!(value["target"]["r_home"], expected);
    assert_eq!(value["selected_by"]["path"], expected);
}

#[cfg(unix)]
fn write_uninstalled_project_override_fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("arf.toml"),
        r#"[experimental]
r_source_overrides = [
  { type = "toml-key", file = "rproject.toml", key = "project.r_version" },
]

[startup]
r_source = { path = "fallback-r-home" }
"#,
    )
    .unwrap();
    std::fs::create_dir(temp.path().join("fallback-r-home")).unwrap();
    // R 3.x ended at 3.6.3, so 3.99.99 can never collide with a real release.
    std::fs::write(
        temp.path().join("rproject.toml"),
        r#"[project]
r_version = "3.99.99"
"#,
    )
    .unwrap();
    temp
}

#[test]
#[cfg(unix)]
fn resolve_uninstalled_project_override_falls_back_to_startup_source() {
    let temp = write_uninstalled_project_override_fixture();
    let environment = FakeRigEnvironment::new();

    let fallback_output = environment
        .command()
        .args([
            "r",
            "resolve",
            "--config",
            "arf.toml",
            "--no-r-source-overrides",
        ])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(
        fallback_output.status.success(),
        "stderr: {:?}",
        fallback_output.stderr
    );
    let fallback_value: serde_json::Value =
        serde_json::from_slice(&fallback_output.stdout).unwrap();
    assert_eq!(fallback_value["resolved"], true);

    let output = environment
        .command()
        .args(["r", "resolve", "--config", "arf.toml"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["resolved"], true);
    assert_eq!(
        value["target"]["r_home"],
        fallback_value["target"]["r_home"]
    );
    assert_eq!(value["selected_by"]["kind"], "startup_r_source");
    assert_ne!(value["selected_by"]["kind"], "r_source_override");

    let diagnostic = value["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|diagnostic| diagnostic["code"] == "r_source_override.version_not_installed")
        .expect("missing uninstalled-version override diagnostic");
    assert_eq!(diagnostic["path"], "rproject.toml");
}

/// Project configuration is a hint that may fall back, while an explicit
/// user request must fail when the requested R version is not installed.
#[test]
#[cfg(unix)]
fn resolve_project_hint_and_explicit_version_have_different_outcomes() {
    let temp = write_uninstalled_project_override_fixture();
    let environment = FakeRigEnvironment::new();

    let project_hint = environment
        .command()
        .args(["r", "resolve", "--config", "arf.toml"])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(
        project_hint.status.success(),
        "stderr: {:?}",
        project_hint.stderr
    );
    let project_hint_value: serde_json::Value =
        serde_json::from_slice(&project_hint.stdout).unwrap();
    assert_eq!(project_hint_value["resolved"], true);
    assert_eq!(
        project_hint_value["selected_by"]["kind"],
        "startup_r_source"
    );

    let explicit_request = environment
        .command()
        .args([
            "r",
            "resolve",
            "--config",
            "arf.toml",
            "--with-r-version",
            "3.99.99",
        ])
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert_structured_resolve_error(&explicit_request, "INVALID_PARAMS");
    assert_eq!(explicit_request.status.code(), Some(2));
}

#[test]
#[cfg(unix)]
fn resolve_environment_version_is_invalid_invocation() {
    let environment = FakeRigEnvironment::new();
    let output = environment
        .command()
        .args(["r", "resolve"])
        .env("ARF_R_VERSION", "3.99.99")
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
#[cfg(not(windows))]
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
        // This test silently no-ops (skips verification) when R is found in the environment.
        // The root cause is that R_LIB_PATHS in crates/arf-libr/src/sys/discovery.rs is a
        // hardcoded constant with no injection point; restricting PATH does not prevent it
        // from being picked up. Integration tests spawn the real binary as a child process,
        // so Rust closure-based dependency injection cannot help here. Making this deterministic
        // requires a production-code mechanism to disable the default path search.
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
#[cfg(unix)]
fn resolve_invalid_version_is_invalid_invocation() {
    let environment = FakeRigEnvironment::new();
    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "not-a-version"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
#[cfg(unix)]
fn resolve_valid_but_uninstalled_version_is_invalid_invocation() {
    let environment = FakeRigEnvironment::new();

    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "3.99.99"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("3.99.99")
    );
}

#[test]
#[cfg(not(windows))]
fn resolve_version_without_rig_is_invalid_invocation() {
    let environment = RLessEnvironment::new();
    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "4.5.2"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
}

#[test]
#[cfg(unix)]
fn resolve_rig_alias_is_accepted_before_version_spec_validation() {
    let environment = FakeRigEnvironment::new();
    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "my-proj"])
        .output()
        .unwrap();

    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["target"]["resolved_version"], "4.4.2");
    assert_eq!(value["selected_by"]["requested_version"], "my-proj");
}

#[test]
#[cfg(unix)]
fn resolve_malformed_rig_output_is_internal_error() {
    let environment = FakeRigEnvironment::new();
    std::fs::write(
        &environment.rig,
        r#"#!/bin/sh
if [ "$1" = "--version" ]; then exit 0; fi
if [ "$1" = "list" ]; then printf '%s\n' 'not json'; exit 0; fi
exit 1
"#,
    )
    .unwrap();

    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "4.4.2"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INTERNAL_ERROR");
    assert_eq!(output.status.code(), Some(4));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("failed to parse rig output")
    );
}

#[test]
#[cfg(unix)]
fn resolve_r_binary_failure_is_internal_error() {
    let environment = FakeRigEnvironment::new();
    std::fs::write(&environment.r, "#!/bin/sh\nexit 1\n").unwrap();

    let output = environment
        .command()
        .args(["r", "resolve", "--with-r-version", "4.4.2"])
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INTERNAL_ERROR");
    assert_eq!(output.status.code(), Some(4));
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("RHOME failed")
    );
}

#[test]
fn resolve_missing_r_home_returns_invalid_params_exit_code() {
    let temp = tempfile::tempdir().unwrap();
    let missing_r_home = temp.path().join("missing-r-home");
    let output = Command::new(env!("CARGO_BIN_EXE_arf"))
        .args(["r", "resolve", "--r-home"])
        .arg(missing_r_home)
        .output()
        .unwrap();

    assert_structured_resolve_error(&output, "INVALID_PARAMS");
    assert_eq!(output.status.code(), Some(2));
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
