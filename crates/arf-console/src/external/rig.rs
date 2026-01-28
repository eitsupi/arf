//! Integration with rig (R Installation Manager).
//!
//! This module provides functions to detect and manage R versions
//! using rig when available.

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
/// - `"4.5"` - Match version starting with "4.5"
/// - `"4.5.2"` - Match exact version
pub fn resolve_version(spec: &str) -> Result<ResolvedVersion, RigError> {
    let versions = list_versions()?;

    if versions.is_empty() {
        return Err(RigError::NoVersionsInstalled);
    }

    let version = match spec.to_lowercase().as_str() {
        "default" => {
            // Find the default version
            versions
                .into_iter()
                .find(|v| v.default)
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
            // Then try prefix match (e.g., "4" matches "4.5.2", selecting highest version)
            else {
                let mut matches: Vec<_> = versions
                    .iter()
                    .filter(|v| v.version.starts_with(spec))
                    .collect();

                if !matches.is_empty() {
                    // Sort by version (highest first) using semver comparison
                    matches.sort_by(|a, b| {
                        let va = parse_version(&a.version);
                        let vb = parse_version(&b.version);
                        vb.cmp(&va) // Reverse order (highest first)
                    });
                    matches[0].clone()
                } else {
                    return Err(RigError::VersionNotFound(spec.to_string()));
                }
            }
        }
    };

    // Get actual R_HOME by running the R binary with RHOME
    // rig's "path" is the installation prefix, not R_HOME
    let r_home = get_r_home_from_binary(&version.binary)?;

    Ok(ResolvedVersion {
        r_home,
        version: version.version,
    })
}

/// Get R_HOME by running `<R binary> RHOME`.
fn get_r_home_from_binary(binary_path: &str) -> Result<String, RigError> {
    let output = Command::new(binary_path)
        .arg("RHOME")
        .output()
        .map_err(|e| RigError::CommandFailed(format!("Failed to run {} RHOME: {}", binary_path, e)))?;

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
