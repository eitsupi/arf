//! Version specification parsing and matching.

use semver::{Version, VersionReq};
use std::fmt;

/// A parsed R version specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpec {
    /// Numeric components pin the precision supplied by the caller.
    Digits(Vec<u64>),
    /// A version range requirement.
    Range(VersionReq),
    /// A named version selector that requires caller-specific handling.
    Named(String),
}

/// An error returned when a version specification cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionSpecParseError {
    /// The specification is empty.
    Empty,
    /// The specification is not a valid numeric prefix, name, or version range requirement.
    Invalid,
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

    pub(super) fn matches(&self, version: &Version) -> bool {
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
