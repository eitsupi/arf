#[cfg(not(windows))]
use std::path::PathBuf;
use std::process::Command;

use crate::common::sanitized_arf_command;
#[cfg(unix)]
use crate::common::write_executable;

#[cfg(not(windows))]
pub(super) struct RLessEnvironment {
    _temp: tempfile::TempDir,
    bin_dir: PathBuf,
    pub(super) fake_r_home: PathBuf,
}

#[cfg(unix)]
pub(super) struct FakeRigEnvironment {
    _temp: tempfile::TempDir,
    bin_dir: PathBuf,
    fake_r_home: PathBuf,
    pub(super) rig: PathBuf,
    pub(super) r: PathBuf,
}

#[cfg(unix)]
impl FakeRigEnvironment {
    pub(super) fn new() -> Self {
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
        )
        .unwrap();

        let rig = bin_dir.join("rig");
        write_executable(
            &rig,
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then exit 0; fi
if [ "$1" = "list" ]; then printf '%s\n' '[{"name":"4.4.2","default":true,"version":"4.4.2","aliases":["my-proj"],"path":"/fake/R/4.4.2","binary":"R"} ]'; exit 0; fi
exit 1
"#,
        )
        .unwrap();

        Self {
            _temp: temp,
            bin_dir,
            fake_r_home,
            rig,
            r,
        }
    }

    pub(super) fn command(&self) -> Command {
        let mut command = isolated_command(self._temp.path());
        command
            .env("PATH", &self.bin_dir)
            .env("FAKE_R_HOME", &self.fake_r_home)
            .env_remove("R_HOME");
        command
    }
}

/// Build a resolve command that cannot see the developer's global arf config.
pub(super) fn isolated_command(temp_path: &std::path::Path) -> Command {
    let home_dir = temp_path.join("home");
    std::fs::create_dir_all(&home_dir).unwrap();
    let mut command = sanitized_arf_command();
    command
        .env("HOME", home_dir)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("XDG_STATE_HOME")
        .env_remove("XDG_CACHE_HOME");
    command
}

#[cfg(not(windows))]
impl RLessEnvironment {
    pub(super) fn new() -> Self {
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
        write_executable(
            &bin_dir.join("R"),
            r#"#!/bin/sh
exit 1
"#,
        )
        .unwrap();

        Self {
            _temp: temp,
            bin_dir,
            fake_r_home,
        }
    }

    /// Build a command that sees neither an R installation nor the developer's
    /// own configuration.
    ///
    /// Pointing `HOME` at an empty directory and clearing the XDG variables
    /// isolates the global `arf.toml`, which `dirs::config_dir()` resolves from
    /// `$XDG_CONFIG_HOME` or `$HOME` depending on the platform. Without this, a
    /// developer or CI image that configures `startup.r_source` would resolve an
    /// R installation and make these tests environment-dependent. The
    /// configuration cannot be isolated with `--config` here, because arf
    /// rejects that flag when it appears before a subcommand and this helper
    /// does not know which subcommand the caller will add.
    pub(super) fn command(&self) -> Command {
        let mut command = isolated_command(self._temp.path());
        #[cfg(unix)]
        let path_entries = vec![self.bin_dir.clone()];
        #[cfg(not(unix))]
        let path_entries = vec![self.bin_dir.clone()];
        command
            .env_remove("R_HOME")
            .env("PATH", std::env::join_paths(path_entries).unwrap());
        command
    }

    pub(super) fn startup_command(&self) -> Command {
        let mut command = self.command();
        command.env("R_HOME", &self.fake_r_home);
        command
    }
}

#[cfg(unix)]
pub(super) fn write_uninstalled_project_override_fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("arf.toml"),
        r#"[experimental]
r_source_overrides = [
  { provider = "toml-key", file = "rproject.toml", key = "project.r_version" },
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
