//! Parsing and resolving R version specifications.
//!
//! This module is intentionally independent from the rest of the application
//! so it can be extracted into a separate crate later.

use semver::{Version, VersionReq};
use std::fmt;

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
    Invalid(String),
}

impl fmt::Display for VersionSpecParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("version specification must not be empty"),
            Self::Invalid(spec) => write!(formatter, "invalid version specification: {spec}"),
        }
    }
}

impl std::error::Error for VersionSpecParseError {}

impl VersionSpec {
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
            let digits = components
                .iter()
                .map(|component| component.parse::<u64>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| VersionSpecParseError::Invalid(input.to_owned()))?;
            return Ok(Self::Digits(digits));
        }

        VersionReq::parse(input)
            .map(Self::Range)
            .map_err(|_| VersionSpecParseError::Invalid(input.to_owned()))
    }

    fn matches(&self, version: &Version) -> bool {
        match self {
            Self::Digits(digits) => {
                let version_components = [version.major, version.minor, version.patch];
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
}
