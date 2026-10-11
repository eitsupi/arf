//! CLI contracts for strict rig-project selection using native fake commands.

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

fn probe_bytes() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory
            .path()
            .join(format!("rig-project-probe{}", std::env::consts::EXE_SUFFIX));
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rig_project_probe.rs");
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
    home: PathBuf,
    wrapper: PathBuf,
    manifest: PathBuf,
    config: PathBuf,
    bin: PathBuf,
    status: PathBuf,
    status_cwd: PathBuf,
    wrapper_cwd: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let manifest = root_path.join("rproj.toml");
        std::fs::write(&manifest, "[project]\nname = 'fixture'\n").unwrap();
        let wrapper = root_path
            .join(".rvenv/bin")
            .join(if cfg!(windows) { "R.exe" } else { "R" });
        std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
        std::fs::write(&wrapper, probe_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let bin = root_path.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake_rig = bin.join(format!("rig{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&fake_rig, probe_bytes()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_rig, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let home = root_path.join("r-installation");
        let library = arf_libr::r_library_path(&home);
        std::fs::create_dir_all(library.parent().unwrap()).unwrap();
        std::fs::write(&library, "deliberately unloadable shared library").unwrap();
        let config = root_path.join("arf.toml");
        std::fs::write(&config, "[history]\nmode = 'volatile'\n").unwrap();
        let status = root_path.join("rig-status.json");
        std::fs::write(
            &status,
            serde_json::to_vec(&status_value(&root_path)).unwrap(),
        )
        .unwrap();
        let status_cwd = root_path.join("status-cwd");
        let wrapper_cwd = root_path.join("wrapper-cwd");

        Self {
            root,
            home,
            wrapper,
            manifest,
            config,
            bin,
            status,
            status_cwd,
            wrapper_cwd,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_arf"));
        command
            .args(args)
            .arg("--config")
            .arg(&self.config)
            .current_dir(self.root.path())
            .env("R_HOME", "/inherited/unrelated/R_HOME")
            .env("ARF_TEST_RIG_R_HOME", &self.home)
            .env("ARF_TEST_RIG_STATUS_FILE", &self.status)
            .env("ARF_TEST_RIG_STATUS_CWD", &self.status_cwd)
            .env("ARF_TEST_RIG_WRAPPER_CWD", &self.wrapper_cwd)
            .env_remove("ARF_R_HOME")
            .env_remove("ARF_R_VERSION")
            .env_remove("ARF_R_PROVIDER")
            .env_remove("ARF_CONFIG")
            .env_remove("RUST_LOG")
            .env_remove("_ARF_INTERNAL_PIXI_LOADER_TARGET")
            .env_remove("_ARF_INTERNAL_R_SOURCE_LOADER_TARGET")
            .env(
                "PATH",
                std::env::join_paths(std::iter::once(self.bin.clone()).chain(
                    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
                ))
                .unwrap(),
            );
        command
    }

    fn write_status(&self, value: &Value) {
        std::fs::write(&self.status, serde_json::to_vec(value).unwrap()).unwrap();
    }

    fn descriptor(&self, args: &[&str]) -> Value {
        let output = self.command(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

fn status_value(root: &Path) -> Value {
    serde_json::json!({
        "root": root,
        "manifest": {"name": "fixture", "version": "0.1.0"},
        "lock": {"version": 1, "targets": []},
        "sync": {"r_version": "4.6-arm64", "up_to_date": true, "reason": null},
        "warnings": [],
        "future_field": "accepted"
    })
}

fn assert_cwd_records(path: &Path, expected: &Path, count: usize) {
    let records = std::fs::read_to_string(path).unwrap();
    let records = records.lines().collect::<Vec<_>>();
    assert_eq!(records.len(), count);
    let expected = expected.canonicalize().unwrap();
    for record in records {
        assert_eq!(PathBuf::from(record).canonicalize().unwrap(), expected);
    }
}

#[test]
fn explicit_cli_and_environment_selection_report_project_metadata() {
    let fixture = Fixture::new();
    let cases = [
        (
            "CLI",
            vec![
                "r",
                "resolve",
                "--r-provider",
                "rig-project",
                "--no-r-source-overrides",
            ],
            None,
            "command_line_argument",
        ),
        (
            "environment",
            vec!["r", "resolve", "--no-r-source-overrides"],
            Some("rig-project"),
            "environment_variable",
        ),
    ];

    for (label, args, provider, source_kind) in cases {
        let mut command = fixture.command(&args);
        if let Some(provider) = provider {
            command.env("ARF_R_PROVIDER", provider);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let descriptor: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(descriptor["provider"], "rig-project", "{label}");
        assert_eq!(
            descriptor["selected_by"]["kind"], "provider_request",
            "{label}"
        );
        assert_eq!(
            descriptor["selected_by"]["requested_provider"], "rig-project",
            "{label}"
        );
        assert_eq!(
            descriptor["selected_by"]["source"]["kind"], source_kind,
            "{label}"
        );
        assert_eq!(descriptor["metadata"]["kind"], "rig-project", "{label}");
        assert_eq!(
            descriptor["metadata"]["root"],
            fixture
                .root
                .path()
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
            "{label}"
        );
        assert_eq!(
            descriptor["metadata"]["manifest"],
            fixture
                .manifest
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
            "{label}"
        );
        assert_eq!(
            descriptor["metadata"]["r_installation"], "4.6-arm64",
            "{label}"
        );
        assert_eq!(
            descriptor["metadata"]["r_binary"],
            fixture
                .wrapper
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
            "{label}"
        );
        assert_eq!(
            descriptor["target"]["r_binary"],
            fixture
                .wrapper
                .canonicalize()
                .unwrap()
                .display()
                .to_string(),
            "{label}"
        );
        assert_eq!(
            descriptor["target"]["resolved_version"],
            Value::Null,
            "{label}"
        );
    }

    assert_cwd_records(&fixture.status_cwd, fixture.root.path(), 2);
    assert_cwd_records(&fixture.wrapper_cwd, fixture.root.path(), 2);
}

#[test]
fn configured_provider_uses_the_same_resolution_and_override_metadata() {
    let fixture = Fixture::new();
    std::fs::write(
        &fixture.config,
        "[experimental]\nr_source_overrides = [{ provider = 'rig-project' }]\n",
    )
    .unwrap();
    let descriptor = fixture.descriptor(&["r", "resolve"]);
    assert_eq!(descriptor["provider"], "rig-project");
    assert_eq!(descriptor["selected_by"]["kind"], "provider_request");
    assert_eq!(
        descriptor["selected_by"]["requested_provider"],
        "rig-project"
    );
    assert_eq!(
        descriptor["selected_by"]["source"]["kind"],
        "configuration_file"
    );
    assert_eq!(
        descriptor["selected_by"]["source"]["path"],
        fixture.config.display().to_string()
    );
    assert_eq!(
        descriptor["metadata"]["manifest"],
        fixture
            .manifest
            .canonicalize()
            .unwrap()
            .display()
            .to_string()
    );
}

#[test]
fn strict_provider_failures_return_reason_codes_without_startup_fallback() {
    let mut errors = Vec::new();
    for (failure, expected_reason) in [
        ("not-project", "r_provider.rig_project_not_found"),
        ("status-unavailable", "r_provider.rig_project_unavailable"),
        ("status-invalid", "r_provider.rig_project_status_invalid"),
        ("not-synced", "r_provider.rig_project_not_synced"),
        ("r-unavailable", "r_provider.rig_project_r_unavailable"),
    ] {
        let fixture = Fixture::new();
        std::fs::write(
            &fixture.config,
            format!(
                "[startup]\nr_source = {{ path = {:?} }}",
                fixture.home.display().to_string()
            ),
        )
        .unwrap();
        let mut command = fixture.command(&[
            "r",
            "resolve",
            "--r-provider",
            "rig-project",
            "--no-r-source-overrides",
        ]);
        match failure {
            "not-project" => std::fs::remove_file(&fixture.manifest).unwrap(),
            "status-unavailable" => {
                command.env("ARF_TEST_RIG_STATUS_FAIL", "1");
            }
            "status-invalid" => std::fs::write(&fixture.status, b"{").unwrap(),
            "not-synced" => {
                let mut status = status_value(fixture.root.path());
                status["warnings"] = serde_json::json!(["lock could not be checked"]);
                fixture.write_status(&status);
            }
            "r-unavailable" => std::fs::remove_file(&fixture.wrapper).unwrap(),
            _ => unreachable!(),
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{failure}");
        assert!(output.stdout.is_empty(), "{failure}: {:?}", output.stdout);
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], "INVALID_PARAMS", "{failure}");
        assert_eq!(
            error["error"]["data"]["reason_code"], expected_reason,
            "{failure}"
        );
        errors.push(serde_json::json!({
            "failure": failure,
            "reason_code": error["error"]["data"]["reason_code"],
        }));
    }
    insta::assert_snapshot!(
        serde_json::to_string_pretty(&errors).unwrap(),
        @r###"
        [
          {
            "failure": "not-project",
            "reason_code": "r_provider.rig_project_not_found"
          },
          {
            "failure": "status-unavailable",
            "reason_code": "r_provider.rig_project_unavailable"
          },
          {
            "failure": "status-invalid",
            "reason_code": "r_provider.rig_project_status_invalid"
          },
          {
            "failure": "not-synced",
            "reason_code": "r_provider.rig_project_not_synced"
          },
          {
            "failure": "r-unavailable",
            "reason_code": "r_provider.rig_project_r_unavailable"
          }
        ]"###
    );
}

#[test]
fn selected_unloadable_project_library_is_fatal_for_repl_script_and_headless() {
    let fixture = Fixture::new();
    for args in [
        &[
            "--r-provider",
            "rig-project",
            "--no-r-source-overrides",
            "--vanilla",
            "--quiet",
        ][..],
        &[
            "--r-provider",
            "rig-project",
            "--no-r-source-overrides",
            "--vanilla",
            "--quiet",
            "-e",
            r#"cat("UNEXPECTED_R_STARTED")"#,
        ][..],
    ] {
        let output = fixture.command(args).output().unwrap();
        assert!(
            !output.status.success(),
            "selected bad lib started: {args:?}"
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

    let headless = fixture
        .command(&[
            "headless",
            "--r-provider",
            "rig-project",
            "--no-r-source-overrides",
            "--vanilla",
            "--quiet",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!headless.status.success());
    assert!(String::from_utf8_lossy(&headless.stderr).contains("Failed to initialize R"));
}
