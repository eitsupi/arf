//! Normalized explicit R source requests from CLI arguments and environment.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RSourceOrigin {
    Cli,
    Environment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExplicitRSource {
    Home(PathBuf),
    Version(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExplicitRSourceRequest {
    pub(crate) source: ExplicitRSource,
    pub(crate) origin: RSourceOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExplicitRSourceError {
    ConflictingCli,
    ConflictingEnvironment,
    InvalidVersionEnvironment,
}

impl std::fmt::Display for ExplicitRSourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ConflictingCli => {
                f.write_str("--r-home and --with-r-version cannot be used together")
            }
            Self::ConflictingEnvironment => {
                f.write_str("ARF_R_HOME and ARF_R_VERSION cannot be used together")
            }
            Self::InvalidVersionEnvironment => {
                f.write_str("ARF_R_VERSION must contain valid Unicode")
            }
        }
    }
}

impl std::error::Error for ExplicitRSourceError {}

/// Resolve CLI and environment values into one explicit source request.
///
/// A CLI value takes precedence over every environment value. Callers should
/// only read environment variables for commands that consume an R source.
pub(crate) fn explicit_r_source_request(
    cli_home: Option<PathBuf>,
    cli_version: Option<String>,
    env_home: Option<OsString>,
    env_version: Option<OsString>,
) -> Result<Option<ExplicitRSourceRequest>, ExplicitRSourceError> {
    if cli_home.is_some() && cli_version.is_some() {
        // clap rejects this combination as well; retain the invariant for
        // callers that construct the normalized request directly.
        return Err(ExplicitRSourceError::ConflictingCli);
    }

    if let Some(home) = cli_home {
        return Ok(Some(ExplicitRSourceRequest {
            source: ExplicitRSource::Home(home),
            origin: RSourceOrigin::Cli,
        }));
    }
    if let Some(version) = cli_version {
        return Ok(Some(ExplicitRSourceRequest {
            source: ExplicitRSource::Version(version),
            origin: RSourceOrigin::Cli,
        }));
    }

    match (env_home, env_version) {
        (Some(_), Some(_)) => Err(ExplicitRSourceError::ConflictingEnvironment),
        (Some(home), None) => Ok(Some(ExplicitRSourceRequest {
            source: ExplicitRSource::Home(PathBuf::from(home)),
            origin: RSourceOrigin::Environment,
        })),
        (None, Some(version)) => {
            let version = version
                .into_string()
                .map_err(|_| ExplicitRSourceError::InvalidVersionEnvironment)?;
            Ok(Some(ExplicitRSourceRequest {
                source: ExplicitRSource::Version(version),
                origin: RSourceOrigin::Environment,
            }))
        }
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_source_precedence_and_conflicts_are_table_driven() {
        let cases = [
            ("default", None, None, None, None, Ok(None)),
            (
                "CLI home wins over both environment sources",
                Some(PathBuf::from("/cli/R")),
                None,
                Some(OsString::from("/env/R")),
                Some(OsString::from("4.5")),
                Ok(Some(ExplicitRSourceRequest {
                    source: ExplicitRSource::Home(PathBuf::from("/cli/R")),
                    origin: RSourceOrigin::Cli,
                })),
            ),
            (
                "CLI version wins over both environment sources",
                None,
                Some("4.4".to_owned()),
                Some(OsString::from("/env/R")),
                Some(OsString::from("not a version")),
                Ok(Some(ExplicitRSourceRequest {
                    source: ExplicitRSource::Version("4.4".to_owned()),
                    origin: RSourceOrigin::Cli,
                })),
            ),
            (
                "environment home is selected",
                None,
                None,
                Some(OsString::from("/env/R")),
                None,
                Ok(Some(ExplicitRSourceRequest {
                    source: ExplicitRSource::Home(PathBuf::from("/env/R")),
                    origin: RSourceOrigin::Environment,
                })),
            ),
            (
                "environment version is selected",
                None,
                None,
                None,
                Some(OsString::from("4.5")),
                Ok(Some(ExplicitRSourceRequest {
                    source: ExplicitRSource::Version("4.5".to_owned()),
                    origin: RSourceOrigin::Environment,
                })),
            ),
            (
                "environment sources conflict",
                None,
                None,
                Some(OsString::from("/env/R")),
                Some(OsString::from("4.5")),
                Err(ExplicitRSourceError::ConflictingEnvironment),
            ),
            (
                "CLI sources conflict",
                Some(PathBuf::from("/cli/R")),
                Some("4.5".to_owned()),
                None,
                None,
                Err(ExplicitRSourceError::ConflictingCli),
            ),
        ];

        for (name, cli_home, cli_version, env_home, env_version, expected) in cases {
            assert_eq!(
                explicit_r_source_request(cli_home, cli_version, env_home, env_version),
                expected,
                "{name}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn environment_source_preserves_non_utf8_home_path() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let path = OsString::from_vec(b"/tmp/R-\xff".to_vec());
        let request = explicit_r_source_request(None, None, Some(path.clone()), None)
            .unwrap()
            .unwrap();
        let ExplicitRSource::Home(home) = request.source else {
            panic!("expected R_HOME source");
        };
        assert_eq!(home.as_os_str().as_bytes(), path.as_os_str().as_bytes());
        assert_eq!(request.origin, RSourceOrigin::Environment);
    }

    #[cfg(windows)]
    #[test]
    fn environment_source_preserves_non_unicode_home_path() {
        use std::os::windows::ffi::OsStringExt;

        let path = OsString::from_wide(&[b'R' as u16, b':' as u16, 0xD800]);
        let request = explicit_r_source_request(None, None, Some(path.clone()), None)
            .unwrap()
            .unwrap();
        let ExplicitRSource::Home(home) = request.source else {
            panic!("expected R_HOME source");
        };
        assert_eq!(home, PathBuf::from(path));
        assert_eq!(request.origin, RSourceOrigin::Environment);
    }

    #[cfg(unix)]
    #[test]
    fn cli_source_ignores_non_utf8_version_environment() {
        use std::os::unix::ffi::OsStringExt;

        let request = explicit_r_source_request(
            Some(PathBuf::from("/cli/R")),
            None,
            None,
            Some(OsString::from_vec(b"4.5-\xff".to_vec())),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            request.source,
            ExplicitRSource::Home(PathBuf::from("/cli/R"))
        );
        assert_eq!(request.origin, RSourceOrigin::Cli);
    }

    #[cfg(unix)]
    #[test]
    fn environment_version_must_be_utf8() {
        use std::os::unix::ffi::OsStringExt;

        assert_eq!(
            explicit_r_source_request(
                None,
                None,
                None,
                Some(OsString::from_vec(b"4.5-\xff".to_vec())),
            ),
            Err(ExplicitRSourceError::InvalidVersionEnvironment)
        );
    }

    #[cfg(windows)]
    #[test]
    fn non_unicode_version_environment_is_rejected_or_ignored_by_precedence() {
        use std::os::windows::ffi::OsStringExt;

        let invalid_version = OsString::from_wide(&[b'4' as u16, b'.' as u16, 0xD800]);
        assert_eq!(
            explicit_r_source_request(None, None, None, Some(invalid_version.clone())),
            Err(ExplicitRSourceError::InvalidVersionEnvironment)
        );

        let request = explicit_r_source_request(
            Some(PathBuf::from(r"C:\cli\R")),
            None,
            None,
            Some(invalid_version),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            request.source,
            ExplicitRSource::Home(PathBuf::from(r"C:\cli\R"))
        );
        assert_eq!(request.origin, RSourceOrigin::Cli);
    }
}
