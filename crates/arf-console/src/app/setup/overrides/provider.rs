//! Provider metadata and file-based version readers for R source overrides.

use super::diagnostics::invalid_override_file_warning;
use crate::app::explicit_r_source::RProvider;
use crate::app::setup::r_source::{RSourceDiagnostic, warning};
use crate::config::RSourceOverride;
use crate::rversion;
use std::path::{Path, PathBuf};

fn resolve_override_path(file: &Path, base_dir: &mut Option<PathBuf>) -> std::io::Result<PathBuf> {
    if let Some(base_dir) = base_dir.as_ref() {
        return Ok(base_dir.join(file));
    }

    let current_dir = std::env::current_dir()?;
    let path = current_dir.join(file);
    *base_dir = Some(current_dir);
    Ok(path)
}

#[derive(Debug, Clone, Copy)]
pub(super) enum FileReader<'a> {
    VersionFile,
    TomlKey { key: &'a str },
    JsonKey { key: &'a str },
}

pub(super) enum ProviderKind<'a> {
    Pixi,
    RigProject,
    File {
        file: &'a Path,
        reader: FileReader<'a>,
    },
}

/// Return the environment provider named by a configured override, if any.
pub(in crate::app::setup) fn configured_provider(source: &RSourceOverride) -> Option<RProvider> {
    match source {
        RSourceOverride::Pixi => Some(RProvider::Pixi),
        RSourceOverride::RigProject => Some(RProvider::RigProject),
        RSourceOverride::VersionFile { .. }
        | RSourceOverride::TomlKey { .. }
        | RSourceOverride::JsonKey { .. } => None,
    }
}

/// Borrowed provider metadata used while resolving one configured override.
///
/// Keeping this separate from `RSourceOverride` centralizes the metadata and
/// file-reading behavior without changing the serde-facing configuration enum.
pub(super) struct ProviderSpec<'a> {
    pub(super) name: &'static str,
    pub(super) kind: ProviderKind<'a>,
}

impl<'a> From<&'a RSourceOverride> for ProviderSpec<'a> {
    fn from(source: &'a RSourceOverride) -> Self {
        match source {
            RSourceOverride::Pixi => Self {
                name: RProvider::Pixi.as_str(),
                kind: ProviderKind::Pixi,
            },
            RSourceOverride::RigProject => Self {
                name: RProvider::RigProject.as_str(),
                kind: ProviderKind::RigProject,
            },
            RSourceOverride::VersionFile { file } => Self {
                name: "version-file",
                kind: ProviderKind::File {
                    file,
                    reader: FileReader::VersionFile,
                },
            },
            RSourceOverride::TomlKey { file, key } => Self {
                name: "toml-key",
                kind: ProviderKind::File {
                    file,
                    reader: FileReader::TomlKey { key },
                },
            },
            RSourceOverride::JsonKey { file, key } => Self {
                name: "json-key",
                kind: ProviderKind::File {
                    file,
                    reader: FileReader::JsonKey { key },
                },
            },
        }
    }
}

impl ProviderSpec<'_> {
    pub(super) fn file_path(&self) -> Option<PathBuf> {
        match &self.kind {
            ProviderKind::Pixi | ProviderKind::RigProject => None,
            ProviderKind::File { file, .. } => Some((*file).to_path_buf()),
        }
    }

    pub(super) fn key(&self) -> Option<&str> {
        match &self.kind {
            ProviderKind::Pixi | ProviderKind::RigProject => None,
            ProviderKind::File { reader, .. } => match reader {
                FileReader::VersionFile => None,
                FileReader::TomlKey { key } | FileReader::JsonKey { key } => Some(key),
            },
        }
    }

    pub(super) fn location(&self) -> String {
        match &self.kind {
            ProviderKind::Pixi | ProviderKind::RigProject => self.name.to_owned(),
            ProviderKind::File { file, reader } => match reader {
                FileReader::VersionFile => file.display().to_string(),
                FileReader::TomlKey { key } | FileReader::JsonKey { key } => {
                    format!("{}:{key}", file.display())
                }
            },
        }
    }
}

pub(super) enum ReadOverride {
    Missing,
    Value(String),
    Rejected(RSourceDiagnostic),
}

/// Read one provider's value and normalize its result for the resolution loop.
pub(super) fn read_override(
    spec: &ProviderSpec<'_>,
    base_dir: &mut Option<PathBuf>,
) -> ReadOverride {
    let ProviderKind::File { file, reader } = &spec.kind else {
        unreachable!("Pixi is evaluated as an active environment provider");
    };
    if !is_bare_filename(file) {
        return ReadOverride::Rejected(invalid_override_file_warning(spec.name, file));
    }

    let path = match resolve_override_path(file, base_dir) {
        Ok(path) => path,
        Err(error) => {
            return ReadOverride::Rejected(warning(
                "r_source_override.value_invalid",
                format!(
                    "Warning: Failed to determine the current directory for R source override file {}: {error}; trying the next R source override.",
                    file.display()
                ),
                Some(file.to_path_buf()),
            ));
        }
    };

    match reader {
        FileReader::VersionFile => read_version_file_override(file, &path),
        FileReader::TomlKey { key } => read_toml_key_override(file, &path, key),
        FileReader::JsonKey { key } => read_json_key_override(file, &path, key),
    }
}

fn read_version_file_override(file: &Path, path: &Path) -> ReadOverride {
    match rversion::read_version_file(path) {
        Ok(version) => ReadOverride::Value(version),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            log::debug!("R source override file {} is not present", file.display());
            ReadOverride::Missing
        }
        Err(error) => ReadOverride::Rejected(warning(
            "r_source_override.value_invalid",
            format!(
                "Warning: Failed to read R version override file {}: {error}; trying the next R source override.",
                file.display()
            ),
            Some(file.to_path_buf()),
        )),
    }
}

fn read_toml_key_override(file: &Path, path: &Path, key: &str) -> ReadOverride {
    match rversion::read_toml_key(path, key) {
        Ok(version) => ReadOverride::Value(version),
        Err(error) if error.is_not_found() => {
            log::debug!("R source override file {} is not present", file.display());
            ReadOverride::Missing
        }
        Err(rversion::TomlKeyError::MissingKey(_) | rversion::TomlKeyError::NotString(_)) => {
            ReadOverride::Rejected(warning(
                "r_source_override.value_invalid",
                format!(
                    "Warning: {}:{} does not contain the configured R version key; trying the next R source override.",
                    file.display(),
                    key
                ),
                Some(file.to_path_buf()),
            ))
        }
        Err(rversion::TomlKeyError::Parse(_)) => ReadOverride::Rejected(warning(
            "r_source_override.value_invalid",
            format!(
                "Warning: Failed to parse R source override file {}; trying the next R source override.",
                file.display()
            ),
            Some(file.to_path_buf()),
        )),
        Err(error) => ReadOverride::Rejected(warning(
            "r_source_override.value_invalid",
            format!(
                "Warning: Failed to read R version from TOML key '{key}' in {}: {error}; trying the next R source override.",
                file.display()
            ),
            Some(file.to_path_buf()),
        )),
    }
}

fn read_json_key_override(file: &Path, path: &Path, key: &str) -> ReadOverride {
    match rversion::read_json_key(path, key) {
        Ok(version) => ReadOverride::Value(version),
        Err(error) if error.is_not_found() => {
            log::debug!("R source override file {} is not present", file.display());
            ReadOverride::Missing
        }
        Err(rversion::JsonKeyError::MissingKey(_) | rversion::JsonKeyError::NotString(_)) => {
            ReadOverride::Rejected(warning(
                "r_source_override.value_invalid",
                format!(
                    "Warning: {}:{} does not contain the configured R version key; trying the next R source override.",
                    file.display(),
                    key
                ),
                Some(file.to_path_buf()),
            ))
        }
        Err(rversion::JsonKeyError::Parse(_)) => ReadOverride::Rejected(warning(
            "r_source_override.value_invalid",
            format!(
                "Warning: Failed to parse R source override file {}; trying the next R source override.",
                file.display()
            ),
            Some(file.to_path_buf()),
        )),
        Err(error) => ReadOverride::Rejected(warning(
            "r_source_override.value_invalid",
            format!(
                "Warning: Failed to read R version from JSON key '{key}' in {}: {error}; trying the next R source override.",
                file.display()
            ),
            Some(file.to_path_buf()),
        )),
    }
}

fn is_bare_filename(file: &Path) -> bool {
    let Some(file) = file.to_str() else {
        return false;
    };

    if file.is_empty()
        || file == "."
        || file == ".."
        || file.contains('/')
        || file.contains('\\')
        || Path::new(file).is_absolute()
    {
        return false;
    }

    let bytes = file.as_bytes();
    !(bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}
