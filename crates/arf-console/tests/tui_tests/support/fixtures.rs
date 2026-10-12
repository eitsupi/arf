use crate::common::{compile_fixture, write_executable};
use anyhow::{Result, bail, ensure};
use std::fs;
use std::sync::OnceLock;

struct FormatterFixture {
    bytes: Vec<u8>,
}

static FORMATTER_FIXTURE: OnceLock<std::result::Result<FormatterFixture, String>> = OnceLock::new();
static RIG_PROJECT_PROBE: OnceLock<std::result::Result<Vec<u8>, String>> = OnceLock::new();

fn compiled_formatter_fixture() -> Result<&'static FormatterFixture> {
    match FORMATTER_FIXTURE
        .get_or_init(|| compile_fixture("formatter").map(|bytes| FormatterFixture { bytes }))
    {
        Ok(fixture) => Ok(fixture),
        Err(error) => bail!("{error}"),
    }
}

/// Create a synchronized rig project whose fake `rig` and project wrapper
/// report `r_home`, and return the environment that selects them.
pub fn build_rig_project_fixture(
    project: &std::path::Path,
    r_home: &std::path::Path,
) -> Result<Vec<(String, String)>> {
    let probe = match RIG_PROJECT_PROBE.get_or_init(|| compile_fixture("rig_project_probe")) {
        Ok(bytes) => bytes,
        Err(error) => bail!("{error}"),
    };
    fs::create_dir_all(project)?;
    let project = project.canonicalize()?;
    fs::write(project.join("rproj.toml"), "[project]\nname = 'fixture'\n")?;
    let wrapper = project
        .join(".rvenv")
        .join("bin")
        .join(format!("R{}", std::env::consts::EXE_SUFFIX));
    write_executable(&wrapper, probe)?;
    let bin = project.join("fake-bin");
    write_executable(
        &bin.join(format!("rig{}", std::env::consts::EXE_SUFFIX)),
        probe,
    )?;
    let status = project.join("rig-status.json");
    fs::write(
        &status,
        serde_json::to_vec(&serde_json::json!({
            "root": project,
            "manifest": {"name": "fixture", "version": "0.1.0"},
            "lock": {"version": 1, "targets": []},
            "sync": {"r_version": "fixture-r", "up_to_date": true, "reason": null},
            "warnings": []
        }))?,
    )?;
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let path =
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&current_path)))?;
    Ok(vec![
        ("PATH".to_owned(), path.to_string_lossy().into_owned()),
        (
            "ARF_TEST_RIG_R_HOME".to_owned(),
            r_home.display().to_string(),
        ),
        (
            "ARF_TEST_RIG_STATUS_FILE".to_owned(),
            status.display().to_string(),
        ),
    ])
}

pub fn build_formatter_fixture_path(
    directory: &std::path::Path,
    formatter: &str,
) -> Result<String> {
    ensure!(
        matches!(formatter, "air" | "arity"),
        "unsupported formatter fixture: {formatter}"
    );
    fs::create_dir_all(directory)?;
    let fixture = directory.join(format!("{formatter}{}", std::env::consts::EXE_SUFFIX));
    fs::write(&fixture, &compiled_formatter_fixture()?.bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fixture, fs::Permissions::from_mode(0o755))?;
    }
    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(directory.to_path_buf()).chain(std::env::split_paths(&current_path)),
    )?;
    Ok(path.to_string_lossy().into_owned())
}
