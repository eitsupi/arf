//! Integration with rig (R Installation Manager).
//!
//! This module provides functions to detect and manage R versions
//! using rig when available.

use crate::rversion;
use serde::Deserialize;
use std::process::Command;

/// Information about an installed R version from rig.
#[derive(Debug, Clone, Deserialize)]
pub struct RigVersion {
    /// Version name (e.g., "4.5.2").
    pub name: String,
    /// Whether this is the default version.
    pub default: bool,
    /// Full version string.
    pub version: String,
    /// Aliases for this version (e.g., ["release"]).
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Path to R installation (R_HOME).
    #[allow(dead_code)]
    pub path: String,
    /// Path to R binary.
    pub binary: String,
}

/// Result of resolving an R version.
#[derive(Debug, Clone)]
pub struct ResolvedVersion {
    /// The R_HOME path.
    pub r_home: String,
    /// The version string.
    pub version: String,
}

/// Check if rig is available in PATH.
pub fn rig_available() -> bool {
    Command::new("rig")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// List all installed R versions via rig.
pub fn list_versions() -> Result<Vec<RigVersion>, RigError> {
    let output = Command::new("rig")
        .args(["list", "--json"])
        .output()
        .map_err(|e| RigError::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RigError::CommandFailed(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Workaround for rig bug on Windows: backslashes in paths are not escaped in JSON.
    // e.g., "C:\Program Files\R" should be "C:\\Program Files\\R"
    // We fix this by escaping backslashes that are followed by characters that would
    // form invalid JSON escape sequences.
    let fixed_json = fix_windows_json_paths(&stdout);

    serde_json::from_str(&fixed_json).map_err(|e| RigError::ParseError(e.to_string()))
}

/// Fix unescaped Windows paths in JSON output from rig.
///
/// rig on Windows outputs paths like "C:\Program Files\R" which contains
/// invalid JSON escapes (\P, \R, \b in \bin, etc.). This function escapes
/// all backslashes that are not already escaped.
///
/// TODO: Remove this workaround once rig fixes the bug:
/// https://github.com/r-lib/rig/issues/308
fn fix_windows_json_paths(json: &str) -> String {
    let mut result = String::with_capacity(json.len() * 2);
    let mut chars = json.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(&next) = chars.peek() {
                if next == '\\' {
                    // Already escaped (\\), keep both backslashes
                    result.push(ch);
                    result.push(chars.next().unwrap());
                } else if next == '"' {
                    // Escaped quote (\"), keep as-is
                    result.push(ch);
                } else {
                    // Unescaped backslash in a Windows path, escape it
                    result.push('\\');
                    result.push('\\');
                }
            } else {
                result.push(ch);
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Resolve a version specification to an R installation.
///
/// # Version specifications
///
/// - `"default"` - Use rig's default version
/// - `"release"` - Use the version aliased as "release"
/// - `"4.5"` - Match the newest installed version in the 4.5 series
/// - `"4.5.2"` - Match exact version
pub fn resolve_version(spec: &str) -> Result<ResolvedVersion, RigError> {
    let versions = list_versions()?;

    resolve_version_from_versions(spec, &versions)
}

/// Resolve a version specification using an already-fetched rig version list.
///
/// This is used by callers that have already called [`list_versions`] and need
/// to avoid fetching the same list again.
fn resolve_version_from_versions(
    spec: &str,
    versions: &[RigVersion],
) -> Result<ResolvedVersion, RigError> {
    resolve_version_from_versions_with(spec, versions, get_r_home_from_binary)
}

/// Resolve an already-selected semantic version from an installed rig list.
///
/// Unlike [`resolve_version_from_versions`], this deliberately matches only
/// the reported version field. It is used by R source overrides after the
/// shared version resolver has selected a semantic version, so rig names and
/// aliases must not reinterpret that selection.
pub fn resolve_selected_version_from_versions(
    selected: &semver::Version,
    versions: &[RigVersion],
) -> Result<ResolvedVersion, RigError> {
    resolve_selected_version_from_versions_with(selected, versions, get_r_home_from_binary)
}

fn resolve_version_from_versions_with<F>(
    spec: &str,
    versions: &[RigVersion],
    get_r_home: F,
) -> Result<ResolvedVersion, RigError>
where
    F: FnOnce(&str) -> Result<String, RigError>,
{
    let version = select_version_from_versions(spec, versions)?;

    resolve_rig_version_with(&version, get_r_home)
}

fn resolve_selected_version_from_versions_with<F>(
    selected: &semver::Version,
    versions: &[RigVersion],
    get_r_home: F,
) -> Result<ResolvedVersion, RigError>
where
    F: FnOnce(&str) -> Result<String, RigError>,
{
    let version = versions
        .iter()
        .find(|version| parse_version(&version.version).as_ref() == Some(selected))
        .ok_or_else(|| RigError::VersionNotFound(selected.to_string()))?;

    resolve_rig_version_with(version, get_r_home)
}

fn resolve_rig_version_with<F>(
    version: &RigVersion,
    get_r_home: F,
) -> Result<ResolvedVersion, RigError>
where
    F: FnOnce(&str) -> Result<String, RigError>,
{
    // Resolve R_HOME from the selected installation.
    let r_home = get_r_home(&version.binary)?;

    Ok(ResolvedVersion {
        r_home,
        version: version.version.clone(),
    })
}

fn select_version_from_versions(
    spec: &str,
    versions: &[RigVersion],
) -> Result<RigVersion, RigError> {
    if versions.is_empty() {
        return Err(RigError::NoVersionsInstalled);
    }

    let version = match spec.to_lowercase().as_str() {
        "default" => {
            // Find the default version
            versions
                .iter()
                .find(|v| v.default)
                .cloned()
                .ok_or(RigError::NoDefaultVersion)?
        }
        _ => {
            // Try to match by alias first
            if let Some(v) = versions
                .iter()
                .find(|v| v.aliases.iter().any(|a| a.eq_ignore_ascii_case(spec)))
            {
                v.clone()
            }
            // Then try exact name match
            else if let Some(v) = versions.iter().find(|v| v.name == spec) {
                v.clone()
            }
            // Then try version match
            else if let Some(v) = versions.iter().find(|v| v.version == spec) {
                v.clone()
            }
            // Finally, use the shared version-spec matching semantics.
            else {
                let parsed_spec = match rversion::VersionSpec::parse(spec) {
                    Ok(parsed_spec) => parsed_spec,
                    Err(_) => return Err(RigError::VersionNotFound(spec.to_string())),
                };
                let installed_versions = versions
                    .iter()
                    .filter_map(|version| parse_version(&version.version))
                    .collect::<Vec<_>>();

                let Some(selected) = rversion::resolve_version(&parsed_spec, &installed_versions)
                else {
                    return Err(RigError::VersionNotFound(spec.to_string()));
                };

                versions
                    .iter()
                    .find(|version| parse_version(&version.version).as_ref() == Some(selected))
                    .cloned()
                    .ok_or_else(|| RigError::VersionNotFound(spec.to_string()))?
            }
        }
    };

    Ok(version)
}

/// Get R_HOME by running `<R binary> RHOME`.
fn get_r_home_from_binary(binary_path: &str) -> Result<String, RigError> {
    let output = Command::new(binary_path)
        .arg("RHOME")
        .output()
        .map_err(|e| {
            RigError::CommandFailed(format!("Failed to run {} RHOME: {}", binary_path, e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(RigError::CommandFailed(format!(
            "{} RHOME failed: {}",
            binary_path, stderr
        )));
    }

    let r_home = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if r_home.is_empty() {
        return Err(RigError::CommandFailed(format!(
            "{} RHOME returned empty result",
            binary_path
        )));
    }

    Ok(r_home)
}

/// Parse a version string into a semver::Version.
/// Handles versions like "4.5.2" by parsing them directly.
fn parse_version(s: &str) -> Option<semver::Version> {
    semver::Version::parse(s).ok()
}

/// Errors that can occur when interacting with rig.
#[derive(Debug, Clone)]
pub enum RigError {
    /// rig command failed to execute.
    CommandFailed(String),
    /// Failed to parse rig output.
    ParseError(String),
    /// No R versions are installed via rig.
    NoVersionsInstalled,
    /// No default R version is set.
    NoDefaultVersion,
    /// Requested R version was not found.
    VersionNotFound(String),
}

impl std::fmt::Display for RigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RigError::CommandFailed(msg) => write!(f, "rig command failed: {}", msg),
            RigError::ParseError(msg) => write!(f, "failed to parse rig output: {}", msg),
            RigError::NoVersionsInstalled => write!(f, "no R versions installed via rig"),
            RigError::NoDefaultVersion => write!(f, "no default R version set in rig"),
            RigError::VersionNotFound(v) => write!(f, "R version '{}' not found", v),
        }
    }
}

impl std::error::Error for RigError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rig_available() {
        // Just check it doesn't panic - result depends on environment
        let _ = rig_available();
    }

    #[test]
    fn test_parse_rig_json() {
        let json = r#"[
            {
                "name": "4.5.2",
                "default": true,
                "version": "4.5.2",
                "aliases": ["release"],
                "path": "/opt/R/4.5.2",
                "binary": "/opt/R/4.5.2/bin/R"
            },
            {
                "name": "4.4.0",
                "default": false,
                "version": "4.4.0",
                "aliases": [],
                "path": "/opt/R/4.4.0",
                "binary": "/opt/R/4.4.0/bin/R"
            }
        ]"#;

        let versions: Vec<RigVersion> = serde_json::from_str(json).unwrap();
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].name, "4.5.2");
        assert!(versions[0].default);
        assert_eq!(versions[0].aliases, vec!["release"]);
        assert_eq!(versions[1].name, "4.4.0");
        assert!(!versions[1].default);
    }

    #[test]
    fn test_parse_version() {
        // Basic parsing
        let v = parse_version("4.5.2").unwrap();
        assert_eq!(v.major, 4);
        assert_eq!(v.minor, 5);
        assert_eq!(v.patch, 2);

        // Version comparison
        let v1 = parse_version("4.5.2").unwrap();
        let v2 = parse_version("4.4.3").unwrap();
        assert!(v1 > v2);

        // Two-digit minor versions (4.10 > 4.9)
        let v3 = parse_version("4.10.0").unwrap();
        let v4 = parse_version("4.9.0").unwrap();
        assert!(v3 > v4);
    }

    #[test]
    fn resolve_version_from_versions_uses_supplied_list_without_process_spawns() {
        let versions = vec![
            RigVersion {
                name: "4.4.1".to_string(),
                default: false,
                version: "4.4.1".to_string(),
                aliases: Vec::new(),
                path: "/opt/R/4.4.1".to_string(),
                binary: "/opt/R/4.4.1/bin/R".to_string(),
            },
            RigVersion {
                name: "4.4.10".to_string(),
                default: true,
                version: "4.4.10".to_string(),
                aliases: vec!["release".to_string()],
                path: "/opt/R/4.4.10".to_string(),
                binary: "/opt/R/4.4.10/bin/R".to_string(),
            },
        ];

        let resolved = resolve_version_from_versions_with("4.4.1", &versions, |binary| {
            assert_eq!(binary, "/opt/R/4.4.1/bin/R");
            Ok("/opt/R/4.4.1/lib/R".to_string())
        })
        .unwrap();

        assert_eq!(resolved.version, "4.4.1");
        assert_eq!(resolved.r_home, "/opt/R/4.4.1/lib/R");
    }

    #[test]
    fn selected_version_ignores_conflicting_rig_names_and_aliases() {
        let versions = vec![
            rig_version("4.4.1", false, "4.5.0", &["4.4.1"]),
            rig_version("installed-4.4.1", false, "4.4.1", &[]),
        ];
        let selected = semver::Version::parse("4.4.1").unwrap();

        let resolved =
            resolve_selected_version_from_versions_with(&selected, &versions, |binary| {
                assert_eq!(binary, "/opt/R/installed-4.4.1/bin/R");
                Ok("/opt/R/installed-4.4.1/lib/R".to_string())
            })
            .unwrap();

        assert_eq!(resolved.version, "4.4.1");
        assert_eq!(resolved.r_home, "/opt/R/installed-4.4.1/lib/R");
    }

    fn rig_version(name: &str, default: bool, version: &str, aliases: &[&str]) -> RigVersion {
        RigVersion {
            name: name.to_string(),
            default,
            version: version.to_string(),
            aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
            path: format!("/opt/R/{name}"),
            binary: format!("/opt/R/{name}/bin/R"),
        }
    }

    #[test]
    fn version_spec_does_not_use_digit_boundary_prefix_matching() {
        let versions = vec![rig_version("4.4.10", false, "4.4.10", &[])];

        let result =
            resolve_version_from_versions_with("4.4.1", &versions, |_| Ok("unused".to_string()));

        assert!(matches!(
            result,
            Err(RigError::VersionNotFound(spec)) if spec == "4.4.1"
        ));
    }

    #[test]
    fn partial_numeric_version_selects_newest_matching_version() {
        let versions = vec![
            rig_version("4.4.1", false, "4.4.1", &[]),
            rig_version("4.4.10", false, "4.4.10", &[]),
            rig_version("4.5.0", false, "4.5.0", &[]),
        ];

        let resolved = resolve_version_from_versions_with("4.4", &versions, |binary| {
            assert_eq!(binary, "/opt/R/4.4.10/bin/R");
            Ok("/opt/R/4.4.10/lib/R".to_string())
        })
        .unwrap();

        assert_eq!(resolved.version, "4.4.10");
    }

    #[test]
    fn rig_specific_selectors_keep_their_existing_behavior() {
        let versions = vec![
            rig_version("4.4.1", false, "4.4.1", &[]),
            rig_version("custom-name", false, "4.4.2", &[]),
            rig_version("4.5.0", true, "4.5.0", &["release"]),
        ];

        assert_eq!(
            select_version_from_versions("default", &versions)
                .unwrap()
                .name,
            "4.5.0"
        );
        assert_eq!(
            select_version_from_versions("RELEASE", &versions)
                .unwrap()
                .name,
            "4.5.0"
        );
        assert_eq!(
            select_version_from_versions("custom-name", &versions)
                .unwrap()
                .name,
            "custom-name"
        );
        assert_eq!(
            select_version_from_versions("4.4.1", &versions)
                .unwrap()
                .name,
            "4.4.1"
        );
    }

    #[test]
    fn semver_ranges_select_the_newest_matching_version() {
        let versions = vec![
            rig_version("4.3.9", false, "4.3.9", &[]),
            rig_version("4.4.0", false, "4.4.0", &[]),
            rig_version("4.4.5", false, "4.4.5", &[]),
            rig_version("4.5.0", false, "4.5.0", &[]),
            rig_version("5.0.0", false, "5.0.0", &[]),
        ];

        for spec in ["^4.4", ">=4.3, <5.0"] {
            let resolved = resolve_version_from_versions_with(spec, &versions, |binary| {
                assert_eq!(binary, "/opt/R/4.5.0/bin/R");
                Ok("/opt/R/4.5.0/lib/R".to_string())
            })
            .unwrap();

            assert_eq!(resolved.version, "4.5.0");
        }
    }

    #[test]
    fn trailing_dot_and_unmatched_named_specs_do_not_match() {
        let versions = vec![rig_version("4.4.10", false, "4.4.10", &[])];

        for spec in ["4.4.", "release", "devel"] {
            let result = select_version_from_versions(spec, &versions);
            assert!(matches!(
                result,
                Err(RigError::VersionNotFound(found)) if found == spec
            ));
        }
    }

    #[test]
    fn test_fix_windows_json_paths() {
        // Simulates rig output on Windows with unescaped backslashes
        let broken_json = r#"[
  {
    "name": "4.5.2",
    "default": true,
    "version": "4.5.2",
    "aliases": ["release"],
    "path": "C:\Program Files\R\R-4.5.2",
    "binary": "C:\Program Files\R\R-4.5.2\bin\R.exe"
  }
]"#;

        let fixed = fix_windows_json_paths(broken_json);
        let versions: Vec<RigVersion> = serde_json::from_str(&fixed).unwrap();

        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].name, "4.5.2");
        assert_eq!(versions[0].path, r"C:\Program Files\R\R-4.5.2");
        assert_eq!(versions[0].binary, r"C:\Program Files\R\R-4.5.2\bin\R.exe");
    }

    #[test]
    fn test_fix_windows_json_paths_preserves_already_escaped() {
        // JSON with already-escaped backslashes should not be double-escaped
        let valid_json = r#"{"path": "C:\\Program Files\\R"}"#;
        let fixed = fix_windows_json_paths(valid_json);
        // Already escaped backslashes should be preserved
        assert!(fixed.contains(r#"C:\\Program Files\\R"#));
        // Should not become quadruple backslashes
        assert!(!fixed.contains(r#"C:\\\\Program"#));
    }

    #[test]
    fn test_fix_windows_json_paths_preserves_escaped_quotes() {
        // Escaped quotes should be preserved
        let json_with_quote = r#"{"name": "test\"value"}"#;
        let fixed = fix_windows_json_paths(json_with_quote);
        assert!(fixed.contains(r#"test\"value"#));
    }
}
