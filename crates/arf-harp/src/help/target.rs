//! Interpretation of the opaque destinations emitted for Rd cross-references.

const HELP_URI_SCHEME: &str = "x-r-help";

pub(super) fn rd_link_options() -> rd2qmd_core::LinkOptions {
    rd2qmd_core::LinkOptions {
        // The empty package slot distinguishes topics containing slashes
        // (such as %/%) from package-qualified targets.
        unqualified_link_url: Some(format!("{HELP_URI_SCHEME}:/{{topic}}")),
        external_link_url: Some(format!("{HELP_URI_SCHEME}:{{package}}/{{topic}}")),
        ..Default::default()
    }
}

/// An R help alias or database key, optionally qualified by package name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelpTarget {
    pub package: Option<String>,
    pub topic: String,
}

impl HelpTarget {
    /// Parse arf's internal help URI format.
    ///
    /// Unqualified targets use `x-r-help:/{topic}`; qualified targets use
    /// `x-r-help:{package}/{topic}`. Only the first slash is a delimiter, so
    /// operator aliases such as `%/%` and `/` retain their original spelling.
    /// Topics are opaque: percent escapes, query markers, and fragment markers
    /// are literal alias characters, not URL components.
    pub fn from_uri(uri: &str) -> Option<Self> {
        let (scheme, target) = uri.split_once(':')?;
        if !scheme.eq_ignore_ascii_case(HELP_URI_SCHEME) || target.chars().any(char::is_control) {
            return None;
        }

        let (package, topic) = if let Some(topic) = target.strip_prefix('/') {
            (None, topic)
        } else {
            let (package, topic) = target.split_once('/')?;
            // Installed R package names start with an ASCII letter and contain
            // only letters, digits, and dots. Do not infer a package from an
            // operator alias in a malformed, delimiter-free URI.
            if !package.starts_with(|c: char| c.is_ascii_alphabetic())
                || !package
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.')
            {
                return None;
            }
            (Some(package.to_owned()), topic)
        };
        if topic.is_empty() {
            return None;
        }
        Some(Self {
            package,
            topic: topic.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::HelpTarget;

    #[test]
    fn unqualified_aliases_are_opaque() {
        for topic in [
            "mean",
            "[",
            "[[",
            "%/%",
            "%in%",
            "/",
            "日本語",
            "foo bar",
            "%2F",
            "?#",
        ] {
            assert_eq!(
                HelpTarget::from_uri(&format!("x-r-help:/{topic}")),
                Some(HelpTarget {
                    package: None,
                    topic: topic.to_owned()
                }),
            );
        }
    }

    #[test]
    fn qualified_aliases_split_at_only_the_first_slash() {
        for (package, topic) in [
            ("stats", "lm"),
            ("base", "%/%"),
            ("base", "/"),
            ("data.table", "["),
            ("pkg2", "path/with/slashes"),
        ] {
            assert_eq!(
                HelpTarget::from_uri(&format!("x-r-help:{package}/{topic}")),
                Some(HelpTarget {
                    package: Some(package.to_owned()),
                    topic: topic.to_owned()
                }),
            );
        }
        assert_eq!(
            HelpTarget::from_uri("X-R-HELP:/mean"),
            HelpTarget::from_uri("x-r-help:/mean")
        );
    }

    #[test]
    fn malformed_or_foreign_uris_are_not_help_targets() {
        for uri in [
            "",
            "mean",
            "https://example.com/mean",
            "x-r-help:",
            "x-r-help:/",
            "x-r-help:base/",
            "x-r-help:mean",
            "x-r-help:%/%",
            "x-r-help:bad package/mean",
            "x-r-help:pkg_name/mean",
            "x-r-help:../mean",
            "x-r-help:base/mean\n",
            "x-r-help:/foo\0bar",
        ] {
            assert_eq!(HelpTarget::from_uri(uri), None, "{uri:?}");
        }
    }
}
