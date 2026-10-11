//! Provider/consumer contracts with a native fake launcher, without Pixi or R.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn launcher_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory
            .path()
            .join(format!("rhome{}", std::env::consts::EXE_SUFFIX));
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rhome_probe.rs");
        let output = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg(source)
            .args(["--edition=2024", "-D", "warnings", "-o"])
            .arg(&binary)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::read(binary).unwrap()
    })
}

struct Fixture {
    root: tempfile::TempDir,
    prefix: PathBuf,
    binary: PathBuf,
    home: PathBuf,
    manifest: PathBuf,
    config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("detached-prefix");
        let binary = prefix.join(if cfg!(windows) {
            "Scripts/R.exe"
        } else {
            "bin/R"
        });
        let home = prefix.join("lib/R");
        let library = arf_libr::r_library_path(&home);
        std::fs::create_dir_all(library.parent().unwrap()).unwrap();
        std::fs::write(&library, "deliberately unloadable shared library").unwrap();
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, launcher_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let manifest = root.path().join("active-pixi.toml");
        std::fs::write(
            &manifest,
            r"[workspace]
name='active'",
        )
        .unwrap();
        let marker = prefix.join("conda-meta/pixi");
        std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
        std::fs::write(
            &marker,
            serde_json::to_vec(
                &serde_json::json!({"manifest_path": manifest, "environment_name": "analysis"}),
            )
            .unwrap(),
        )
        .unwrap();
        // Different CWD workspace must not supersede activation.
        std::fs::write(
            root.path().join("pixi.toml"),
            r"[workspace]
name='other'",
        )
        .unwrap();
        let config = root.path().join("arf.toml");
        std::fs::write(
            &config,
            r"[experimental]
r_source_overrides=[{provider='pixi'}]",
        )
        .unwrap();
        Self {
            root,
            prefix,
            binary,
            home,
            manifest,
            config,
        }
    }

    fn command(&self, resolve: bool) -> Command {
        if resolve {
            self.command_with_args(&["r", "resolve"])
        } else {
            self.command_with_args(&[])
        }
    }

    fn command_with_args(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_arf"));
        command.args(args);
        command
            .arg("--config")
            .arg(&self.config)
            .current_dir(self.root.path())
            .env("PIXI_IN_SHELL", "")
            .env("PIXI_WORKSPACE_MANIFEST", &self.manifest)
            .env("PIXI_ENVIRONMENT_NAME", "analysis")
            .env("CONDA_PREFIX", &self.prefix)
            .env("R_HOME", "/unrelated/inherited/R_HOME")
            .env("ARF_TEST_RHOME_OUTPUT", &self.home)
            .env_remove("ARF_R_HOME")
            .env_remove("ARF_R_VERSION")
            .env_remove("ARF_R_PROVIDER")
            .env_remove("ARF_CONFIG")
            .env_remove("PIXI_PROJECT_MANIFEST")
            .env_remove("RUST_LOG")
            .env_remove("_ARF_INTERNAL_PIXI_LOADER_TARGET")
            .env(
                "PATH",
                std::env::join_paths(
                    std::iter::once(self.binary.parent().unwrap().to_path_buf()).chain(
                        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
                    ),
                )
                .unwrap(),
            );
        command
    }

    fn descriptor(&self) -> Value {
        let output = self.command(true).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "resolve must report diagnostics in JSON"
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn active_pixi_resolve_preserves_the_launcher_outside_home() {
    let fixture = Fixture::new();
    let descriptor = fixture.descriptor();
    assert_eq!(descriptor["provider"], "pixi");
    assert_eq!(descriptor["selected_by"]["kind"], "active_environment");
    assert_eq!(descriptor["selected_by"]["source"]["kind"], "pixi");
    assert_eq!(descriptor["selected_by"]["source"]["name"], "analysis");
    assert_eq!(
        descriptor["selected_by"]["source"]["path"],
        fixture
            .manifest
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(
        descriptor["target"]["r_binary"],
        fixture.binary.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(
        descriptor["target"]["r_home"],
        fixture.home.canonicalize().unwrap().display().to_string()
    );
    assert!(descriptor["target"]["resolved_version"].is_null());
    assert_eq!(descriptor["diagnostics"], serde_json::json!([]));
    assert_eq!(descriptor["selected_by"]["requested_provider"], Value::Null);
    assert_eq!(descriptor["metadata"]["kind"], "pixi");
    assert_eq!(descriptor["metadata"]["environment"], "analysis");
    assert_eq!(
        descriptor["metadata"]["manifest"],
        fixture
            .manifest
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
    assert_eq!(
        descriptor["metadata"]["prefix"],
        fixture.prefix.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(
        descriptor["metadata"]["r_binary"],
        fixture.binary.canonicalize().unwrap().display().to_string()
    );
}

#[test]
fn explicit_provider_resolve_reports_request_source_without_configured_override() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.config, "").unwrap();
    let cases = [
        (
            "--r-provider",
            vec!["r", "resolve", "--r-provider", "pixi"],
            None,
        ),
        ("ARF_R_PROVIDER", vec!["r", "resolve"], Some("pixi")),
    ];
    let mut provenance = Vec::new();
    for (label, args, env_provider) in cases {
        let mut command = fixture.command_with_args(&args);
        if let Some(provider) = env_provider {
            command.env("ARF_R_PROVIDER", provider);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(descriptor["provider"], "pixi");
        assert_eq!(descriptor["metadata"]["kind"], "pixi");
        assert_eq!(descriptor["metadata"]["environment"], "analysis");
        assert_eq!(
            descriptor["metadata"]["manifest"],
            fixture
                .manifest
                .canonicalize()
                .unwrap()
                .display()
                .to_string()
        );
        assert_eq!(
            descriptor["metadata"]["prefix"],
            fixture.prefix.canonicalize().unwrap().display().to_string()
        );
        assert_eq!(
            descriptor["metadata"]["r_binary"],
            fixture.binary.canonicalize().unwrap().display().to_string()
        );
        assert_eq!(descriptor["selected_by"]["requested_provider"], "pixi");
        assert_eq!(
            descriptor["target"]["r_binary"],
            fixture.binary.canonicalize().unwrap().display().to_string()
        );
        provenance.push(serde_json::json!({
            "provider": descriptor["provider"],
            "selected_by": descriptor["selected_by"],
        }));
    }
    insta::assert_snapshot!(
        serde_json::to_string_pretty(&provenance).unwrap(),
        @r###"
        [
          {
            "provider": "pixi",
            "selected_by": {
              "kind": "provider_request",
              "requested_provider": "pixi",
              "requested_r_home": null,
              "requested_version": null,
              "source": {
                "format": null,
                "key": null,
                "kind": "command_line_argument",
                "name": "--r-provider",
                "path": null
              }
            }
          },
          {
            "provider": "pixi",
            "selected_by": {
              "kind": "provider_request",
              "requested_provider": "pixi",
              "requested_r_home": null,
              "requested_version": null,
              "source": {
                "format": null,
                "key": null,
                "kind": "environment_variable",
                "name": "ARF_R_PROVIDER",
                "path": null
              }
            }
          }
        ]"###
    );
}

#[test]
fn explicit_provider_failures_are_typed_and_never_fall_back() {
    let cases = [
        "r_provider.pixi_not_active",
        "r_provider.pixi_environment_invalid",
        "r_provider.pixi_r_unavailable",
    ];
    let mut errors = Vec::new();

    for reason_code in cases {
        let fixture = Fixture::new();
        std::fs::write(
            &fixture.config,
            r#"[startup]
r_source = "auto""#,
        )
        .unwrap();
        let mut command = fixture.command_with_args(&["r", "resolve", "--r-provider", "pixi"]);
        match reason_code {
            "r_provider.pixi_not_active" => {
                command
                    .env_remove("PIXI_IN_SHELL")
                    .env_remove("PIXI_WORKSPACE_MANIFEST")
                    .env_remove("PIXI_ENVIRONMENT_NAME")
                    .env_remove("CONDA_PREFIX");
            }
            "r_provider.pixi_environment_invalid" => {
                command.env("PIXI_WORKSPACE_MANIFEST", "");
            }
            "r_provider.pixi_r_unavailable" => {
                command.env("ARF_TEST_RHOME_OUTPUT", fixture.root.path());
            }
            _ => unreachable!(),
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{reason_code}");
        assert!(output.stdout.is_empty(), "{reason_code}");
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], "INVALID_PARAMS");
        assert_eq!(error["error"]["data"]["reason_code"], reason_code);
        if reason_code == "r_provider.pixi_not_active" {
            assert!(
                error["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("pixi run")
            );
        }
        errors.push(serde_json::json!({
            "code": error["error"]["code"],
            "hint": error["error"]["hint"],
            "data": error["error"]["data"],
        }));
    }

    insta::assert_snapshot!(
        serde_json::to_string_pretty(&errors).unwrap(),
        @r###"
        [
          {
            "code": "INVALID_PARAMS",
            "data": {
              "reason_code": "r_provider.pixi_not_active"
            },
            "hint": null
          },
          {
            "code": "INVALID_PARAMS",
            "data": {
              "reason_code": "r_provider.pixi_environment_invalid"
            },
            "hint": null
          },
          {
            "code": "INVALID_PARAMS",
            "data": {
              "reason_code": "r_provider.pixi_r_unavailable"
            },
            "hint": null
          }
        ]"###
    );
}

#[test]
fn explicit_source_and_disable_skip_invalid_pixi_identity() {
    let fixture = Fixture::new();
    for args in [
        vec!["--r-home"],
        vec!["--no-r-source-overrides", "--r-home"],
    ] {
        let output = fixture
            .command(true)
            .env("PIXI_WORKSPACE_MANIFEST", "")
            .args(args)
            .arg(&fixture.home)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(descriptor["provider"], "explicit_path");
        assert_eq!(descriptor["diagnostics"], serde_json::json!([]));
    }
}

#[test]
fn unapplied_pixi_diagnostic_is_json_only_and_fallback_is_reported_once() {
    let fixture = Fixture::new();
    std::fs::write(
        &fixture.config,
        format!(
            r"[startup]
r_source={{path={:?}}}
[experimental]
r_source_overrides=[{{provider='pixi'}}]",
            fixture.home.display().to_string()
        ),
    )
    .unwrap();
    let output = fixture
        .command(true)
        .env("PIXI_WORKSPACE_MANIFEST", "")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
    let diagnostics = descriptor["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(
        diagnostics[0]["code"],
        "r_source_override.pixi_environment_invalid"
    );
    assert_eq!(diagnostics[1]["code"], "r_source_override.fallback");
    assert!(
        diagnostics[1]["message"]
            .as_str()
            .unwrap()
            .contains("Using startup.r_source: path")
    );
    assert_eq!(diagnostics[1]["path"], fixture.home.display().to_string());
    assert!(
        diagnostics[1]["message"]
            .as_str()
            .unwrap()
            .contains(&fixture.home.display().to_string())
    );
}

#[test]
fn selected_unloadable_library_never_falls_back_to_system_r() {
    let fixture = Fixture::new();
    for args in [
        &["--vanilla", "--quiet"][..],
        &["--vanilla", "--quiet", "-e", "cat('UNEXPECTED_R_STARTED')"][..],
    ] {
        let output = fixture.command(false).args(args).output().unwrap();
        assert!(
            !output.status.success(),
            "selected dummy library must fail startup: {args:?}"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("UNEXPECTED_R_STARTED"), "{stdout}");
        assert!(!stdout.contains("Edit mode:"), "REPL started: {stdout}");
        assert!(
            !stdout.contains("R is not initialized"),
            "REPL started: {stdout}"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Failed to initialize R"), "{stderr}");
        assert!(!stderr.to_lowercase().contains("falling back"), "{stderr}");
    }
}

#[test]
fn explicit_provider_without_override_config_fails_after_load_error() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.config, "").unwrap();
    let output = fixture
        .command(false)
        .args([
            "--no-r-source-overrides",
            "--r-provider",
            "pixi",
            "--vanilla",
            "--quiet",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Failed to initialize R from the selected Pixi environment"),
        "{stderr}"
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Edit mode:"));

    let script = fixture
        .command(false)
        .args([
            "--no-r-source-overrides",
            "--r-provider",
            "pixi",
            "--vanilla",
            "--quiet",
            "-e",
            r#"cat("UNEXPECTED_R_STARTED")"#,
        ])
        .output()
        .unwrap();
    assert!(!script.status.success());
    assert!(String::from_utf8_lossy(&script.stderr).contains("Failed to initialize R"));
    assert!(!String::from_utf8_lossy(&script.stdout).contains("UNEXPECTED_R_STARTED"));

    let headless = fixture
        .command_with_args(&[
            "headless",
            "--no-r-source-overrides",
            "--r-provider",
            "pixi",
            "--vanilla",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!headless.status.success());
    assert!(String::from_utf8_lossy(&headless.stderr).contains("Failed to initialize R"));
}

#[test]
fn auto_path_discovery_remains_bounded_after_pixi_probe_rejection() {
    assert_auto_path_discovery_is_bounded(&["stdout", "stderr"]);
}

#[test]
#[ignore = "Runs once per OS in the Pixi integration workflow; waits for two production probe timeouts"]
fn auto_path_discovery_times_out_after_pixi_probe_rejection() {
    assert_auto_path_discovery_is_bounded(&["hang"]);
}

fn assert_auto_path_discovery_is_bounded(modes: &[&str]) {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let fixture = Fixture::new();
    std::fs::write(
        &fixture.config,
        r"[startup]
r_source='auto'
[experimental]
r_source_overrides=[{provider='pixi'}]",
    )
    .unwrap();
    for &mode in modes {
        let counter = fixture.root.path().join(format!("{mode}-calls"));
        let start = Instant::now();
        let mut child = fixture
            .command(true)
            // Isolate PATH to exclude rig and any other R executable.
            .env("PATH", fixture.binary.parent().unwrap())
            .env_remove("R_HOME")
            .env("ARF_TEST_RHOME_MODE", mode)
            .env("ARF_TEST_RHOME_COUNT", &counter)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if start.elapsed() > Duration::from_secs(35) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("auto discovery exceeded its probe deadlines: {mode}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{mode}: {output:?}");
        assert!(output.stderr.is_empty(), "{mode}: {output:?}");
        let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
        let diagnostics = descriptor["diagnostics"].as_array().unwrap();
        let rejected = diagnostics
            .iter()
            .find(|diagnostic| diagnostic["code"] == "r_source_override.pixi_r_unavailable")
            .expect("Pixi probe must reject this launcher");
        let reason = if mode == "hang" {
            "time limit"
        } else {
            "output limit"
        };
        assert!(rejected["message"].as_str().unwrap().contains(reason));
        assert_eq!(std::fs::read_to_string(counter).unwrap(), "2", "{mode}");
    }
}
