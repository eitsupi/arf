//! Parsing and resolving R version specifications.
//!
//! This module is intentionally independent from the rest of the application
//! so it can be extracted into a separate crate later.

use semver::{Version, VersionReq};
use std::fmt;
use std::fs::{self, File};
use std::io;
use std::io::{BufRead, Read};
use std::path::Path;

/// A parsed R version specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpec {
    /// Numeric components pin the precision supplied by the caller.
    Digits(Vec<u64>),
    /// A semantic-version requirement.
    Range(VersionReq),
    /// A named version selector that requires caller-specific handling.
    Named(String),
}

/// An error returned when a version specification cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpecParseError {
    /// The specification is empty.
    Empty,
    /// The specification is not a valid numeric prefix, name, or semver requirement.
    Invalid,
}

/// An error returned when reading a version from a TOML key.
#[derive(Debug)]
pub enum TomlKeyError {
    /// The TOML file could not be read.
    Read(io::Error),
    /// The TOML document could not be parsed.
    Parse(toml::de::Error),
    /// The requested key path does not exist.
    MissingKey(String),
    /// The requested key does not contain a string.
    NotString(String),
}

impl fmt::Display for TomlKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "failed to read TOML file: {error}"),
            Self::Parse(_) => formatter.write_str("failed to parse TOML"),
            Self::MissingKey(key) => write!(formatter, "TOML key not found: {key}"),
            Self::NotString(key) => write!(formatter, "TOML key is not a string: {key}"),
        }
    }
}

impl std::error::Error for TomlKeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::MissingKey(_) | Self::NotString(_) => None,
        }
    }
}

impl TomlKeyError {
    /// Return whether this error means that the configured file is absent.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Read(error) if error.kind() == io::ErrorKind::NotFound)
    }
}

/// The maximum length of the trimmed value read from a version file.
const MAX_VERSION_FILE_VALUE_LENGTH: usize = 256;

/// Read the first non-empty version specification from a plain text file.
pub fn read_version_file(path: &Path) -> io::Result<String> {
    let file = File::open(path)?;
    let mut reader = io::BufReader::new(file);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader
            .by_ref()
            .take((MAX_VERSION_FILE_VALUE_LENGTH + 1) as u64)
            .read_line(&mut line)?;
        if bytes_read == 0 {
            return Ok(String::new());
        }

        // Reject rather than truncate: truncation could turn an invalid value into a valid one.
        if bytes_read > MAX_VERSION_FILE_VALUE_LENGTH && !line.ends_with('\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("version file value exceeds {MAX_VERSION_FILE_VALUE_LENGTH} bytes"),
            ));
        }

        let value = line.trim();
        if value.is_empty() {
            continue;
        }

        // Reject rather than truncate: truncation could turn an invalid value into a valid one.
        if value.len() > MAX_VERSION_FILE_VALUE_LENGTH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("version file value exceeds {MAX_VERSION_FILE_VALUE_LENGTH} bytes"),
            ));
        }
        return Ok(value.to_owned());
    }
}

/// Read a string value from a dot-separated key path in a TOML file.
pub fn read_toml_key(path: &Path, key: &str) -> Result<String, TomlKeyError> {
    let contents = fs::read_to_string(path).map_err(TomlKeyError::Read)?;
    let document = toml::from_str::<toml::Value>(&contents).map_err(TomlKeyError::Parse)?;

    let mut value = &document;
    for component in key.split('.') {
        if component.is_empty() {
            return Err(TomlKeyError::MissingKey(key.to_owned()));
        }
        value = value
            .get(component)
            .ok_or_else(|| TomlKeyError::MissingKey(key.to_owned()))?;
    }

    value
        .as_str()
        .map(|version| version.trim().to_owned())
        .ok_or_else(|| TomlKeyError::NotString(key.to_owned()))
}

impl fmt::Display for VersionSpecParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("version specification must not be empty"),
            Self::Invalid => formatter.write_str("invalid version specification"),
        }
    }
}

impl std::error::Error for VersionSpecParseError {}

/// The number of components in a `semver::Version`.
const VERSION_COMPONENT_COUNT: usize = 3;

impl VersionSpec {
    /// Return whether this is a numeric version selector rather than a range.
    pub fn is_concrete_version(&self) -> bool {
        matches!(self, Self::Digits(_))
    }

    /// Parse a version specification.
    ///
    /// Plain numeric strings are kept separate from semver parsing because an
    /// operator-less `VersionReq` has implicit caret semantics.
    pub fn parse(input: &str) -> Result<Self, VersionSpecParseError> {
        if input.trim().is_empty() {
            return Err(VersionSpecParseError::Empty);
        }

        let input = input.trim();
        if matches!(input, "devel" | "release") {
            return Ok(Self::Named(input.to_owned()));
        }

        let components = input.split('.').collect::<Vec<_>>();
        if components.iter().all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
        }) {
            // A version has at most major, minor and patch, so a longer
            // specification could never match anything.
            if components.len() > VERSION_COMPONENT_COUNT {
                return Err(VersionSpecParseError::Invalid);
            }
            let digits = components
                .iter()
                .map(|component| component.parse::<u64>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| VersionSpecParseError::Invalid)?;
            return Ok(Self::Digits(digits));
        }

        let requirement = VersionReq::parse(input).map_err(|_| VersionSpecParseError::Invalid)?;

        // R versions have no prerelease identifiers or build metadata, so a
        // requirement containing either could never match an installed R version.
        if input.contains('+')
            || requirement
                .comparators
                .iter()
                .any(|comparator| !comparator.pre.is_empty())
        {
            return Err(VersionSpecParseError::Invalid);
        }

        Ok(Self::Range(requirement))
    }

    fn matches(&self, version: &Version) -> bool {
        match self {
            Self::Digits(digits) => {
                let version_components: [u64; VERSION_COMPONENT_COUNT] =
                    [version.major, version.minor, version.patch];
                digits.len() <= version_components.len()
                    && digits
                        .iter()
                        .zip(version_components)
                        .all(|(requested, installed)| *requested == installed)
            }
            Self::Range(requirement) => requirement.matches(version),
            Self::Named(_) => false,
        }
    }
}

/// Resolve a parsed specification to the highest matching installed version.
pub fn resolve_version<'a>(spec: &VersionSpec, installed: &'a [Version]) -> Option<&'a Version> {
    installed
        .iter()
        .filter(|version| spec.matches(version))
        .max()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions(values: &[&str]) -> Vec<Version> {
        values
            .iter()
            .map(|value| Version::parse(value).unwrap())
            .collect()
    }

    #[test]
    fn patch_precision_does_not_match_a_different_patch() {
        let installed = versions(&["4.4.2"]);
        let spec = VersionSpec::parse("4.4.1").unwrap();

        assert_eq!(resolve_version(&spec, &installed), None);
    }

    #[test]
    fn minor_precision_matches_all_requested_patches_only() {
        let installed = versions(&["4.4.0", "4.4.1", "4.4.2", "4.5.0"]);
        let spec = VersionSpec::parse("4.4").unwrap();

        let selected = resolve_version(&spec, &installed).unwrap();
        assert_eq!(selected, &Version::parse("4.4.2").unwrap());
        assert!(!spec.matches(&Version::parse("4.5.0").unwrap()));
        assert!(spec.matches(&Version::parse("4.4.0").unwrap()));
        assert!(spec.matches(&Version::parse("4.4.1").unwrap()));
        assert!(spec.matches(&Version::parse("4.4.2").unwrap()));
    }

    #[test]
    fn patch_precision_does_not_cross_a_digit_boundary() {
        let installed = versions(&["4.4.10"]);
        let spec = VersionSpec::parse("4.4.1").unwrap();

        assert_eq!(resolve_version(&spec, &installed), None);
    }

    #[test]
    fn major_precision_matches_the_whole_major_series() {
        let installed = versions(&["3.6.3", "4.0.0", "4.4.2", "4.10.0"]);
        let spec = VersionSpec::parse("4").unwrap();

        assert_eq!(
            resolve_version(&spec, &installed).unwrap().to_string(),
            "4.10.0"
        );
        assert!(!spec.matches(&Version::parse("3.6.3").unwrap()));
    }

    #[test]
    fn highest_matching_version_is_selected() {
        let installed = versions(&["4.4.1", "4.4.10", "4.4.2"]);
        let spec = VersionSpec::parse("4.4").unwrap();

        assert_eq!(
            resolve_version(&spec, &installed).unwrap().to_string(),
            "4.4.10"
        );
    }

    #[test]
    fn semver_ranges_are_parsed_and_applied() {
        let installed = versions(&["4.3.9", "4.4.0", "4.4.5", "4.5.0", "5.0.0"]);

        let caret = VersionSpec::parse("^4.4").unwrap();
        assert_eq!(
            resolve_version(&caret, &installed).unwrap().to_string(),
            "4.5.0"
        );
        assert!(!caret.matches(&Version::parse("4.3.9").unwrap()));
        assert!(!caret.matches(&Version::parse("5.0.0").unwrap()));

        let comparison = VersionSpec::parse(">=4.3, <5.0").unwrap();
        assert_eq!(
            resolve_version(&comparison, &installed)
                .unwrap()
                .to_string(),
            "4.5.0"
        );

        let tilde = VersionSpec::parse("~4.4").unwrap();
        assert_eq!(
            resolve_version(&tilde, &installed).unwrap().to_string(),
            "4.4.5"
        );
        assert!(!tilde.matches(&Version::parse("4.5.0").unwrap()));
    }

    #[test]
    fn supported_r_version_specifications_are_accepted() {
        for specification in ["4.4.2", "4.4", "4", "^4.4", ">=4.3, <5.0", "~4.4", "*"] {
            assert!(
                VersionSpec::parse(specification).is_ok(),
                "expected {specification:?} to be accepted"
            );
        }
    }

    #[test]
    fn named_specs_use_a_dedicated_variant() {
        assert_eq!(
            VersionSpec::parse("devel").unwrap(),
            VersionSpec::Named("devel".into())
        );
        assert_eq!(
            VersionSpec::parse("release").unwrap(),
            VersionSpec::Named("release".into())
        );
    }

    #[test]
    fn invalid_specs_are_rejected() {
        assert!(matches!(
            VersionSpec::parse(""),
            Err(VersionSpecParseError::Empty)
        ));
        assert!(VersionSpec::parse("r-4.4.1").is_err());
    }

    #[test]
    fn numeric_specs_longer_than_a_version_are_rejected() {
        assert!(matches!(
            VersionSpec::parse("4.4.1.0"),
            Err(VersionSpecParseError::Invalid)
        ));
    }

    #[test]
    fn prerelease_and_build_metadata_specs_are_rejected() {
        for specification in [
            "4.3.0-SUPER-SECRET-TOKEN",
            ">=4.3.0-alpha",
            "=4.4.2-x",
            ">=4.3.0+SUPERSECRET",
            "^4.4.0+build",
        ] {
            assert!(matches!(
                VersionSpec::parse(specification),
                Err(VersionSpecParseError::Invalid)
            ));
        }
    }

    #[test]
    fn version_file_uses_first_non_empty_line() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "\n  4.4.2\r\nprivate trailing contents\n").unwrap();

        assert_eq!(read_version_file(file.path()).unwrap(), "4.4.2");
    }

    #[test]
    fn version_file_rejects_overlong_first_non_empty_line() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            format!("{}\n", "4".repeat(MAX_VERSION_FILE_VALUE_LENGTH + 1)),
        )
        .unwrap();

        let error = read_version_file(file.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(error.to_string(), "version file value exceeds 256 bytes");
    }

    #[test]
    fn version_file_rejects_an_enormous_single_line() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), format!("{}\n", "4".repeat(1024 * 1024))).unwrap();

        let error = read_version_file(file.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn version_file_rejects_an_enormous_whitespace_only_line() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), format!("{}\n", " ".repeat(1024 * 1024))).unwrap();

        let error = read_version_file(file.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn toml_key_reads_nested_string() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "[project]\nr_version = \"4.4\"\n").unwrap();

        assert_eq!(
            read_toml_key(file.path(), "project.r_version").unwrap(),
            "4.4"
        );
    }

    #[test]
    fn toml_key_reports_missing_key() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "[project]\nname = \"arf\"\n").unwrap();

        assert!(matches!(
            read_toml_key(file.path(), "project.r_version"),
            Err(TomlKeyError::MissingKey(_))
        ));
    }

    #[test]
    fn toml_key_reports_parse_errors() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "[project\n").unwrap();

        assert!(matches!(
            read_toml_key(file.path(), "project.r_version"),
            Err(TomlKeyError::Parse(_))
        ));
    }
}
