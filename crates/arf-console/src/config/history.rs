//! History configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::path::{Path, PathBuf};

/// Where command history is kept for the duration of a process.
///
/// `volatile` deliberately keeps history in an arf-owned in-memory SQLite
/// store. It never reads or writes the configured history directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryMode {
    Persistent { dir: Option<PathBuf> },
    Volatile,
}

/// Origin of the effective on-disk history directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryLocationSource {
    /// The platform default directory.
    Default,
    /// The previous platform default, chosen automatically because it holds
    /// history and the current default does not.
    LegacyDefault,
    Explicit,
    Volatile,
}

impl HistoryLocationSource {
    /// Stable machine-readable name used in JSON output.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::LegacyDefault => "legacy_default",
            Self::Explicit => "explicit",
            Self::Volatile => "volatile",
        }
    }

    /// Human-readable description used in `:info`.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Default => "default location",
            Self::LegacyDefault => "legacy default location",
            Self::Explicit => "explicit directory",
            Self::Volatile => "volatile",
        }
    }
}

/// A failure to inspect a default history directory.
///
/// Probing must not mistake permission or I/O errors for "no history here":
/// that could silently start a new history next to an unreadable old one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryProbeFailure {
    directory: PathBuf,
    message: String,
}

impl HistoryProbeFailure {
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for HistoryProbeFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not check for history databases in {}: {}",
            self.directory.display(),
            self.message
        )
    }
}

/// One resolved history location shared by all history consumers.
///
/// Volatile mode intentionally has no directory. The resolver itself takes the
/// already-derived default directories, keeping environment lookup outside the
/// decision. When the default directories could not be probed, the location has
/// no directory and carries the failure so every consumer reports the same
/// diagnosis instead of guessing a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedHistoryLocation {
    directory: Option<PathBuf>,
    source: HistoryLocationSource,
    probe_failure: Option<HistoryProbeFailure>,
}

impl ResolvedHistoryLocation {
    pub(crate) fn directory(&self) -> Option<&Path> {
        self.directory.as_deref()
    }

    pub(crate) fn source(&self) -> HistoryLocationSource {
        self.source
    }

    pub(crate) fn probe_failure(&self) -> Option<&HistoryProbeFailure> {
        self.probe_failure.as_ref()
    }

    pub(crate) fn database_path(&self, filename: &str) -> Option<PathBuf> {
        self.directory
            .as_ref()
            .map(|directory| directory.join(filename))
    }

    /// Startup warning shown only when the legacy default directory was
    /// selected automatically.
    pub(crate) fn legacy_default_warning(&self) -> Option<String> {
        if self.source != HistoryLocationSource::LegacyDefault {
            return None;
        }
        let directory = self.directory.as_deref()?;
        Some(format!(
            "Using the legacy history location {}; the default location has changed. \
             Run `arf history migrate` to move your history to the new default location.",
            directory.display()
        ))
    }
}

/// Resolve the history directory for one process.
///
/// Explicit and volatile modes never inspect the filesystem. For the default
/// mode, a directory "has history" when it contains `r.db` or `shell.db`: the
/// current default wins when it has history, otherwise the legacy default is
/// used when it has history, otherwise the current default is used. The whole
/// directory is chosen at once so R and Shell histories always agree.
pub(crate) fn resolve_history_location(
    mode: &HistoryMode,
    defaults: Option<&DefaultHistoryDirs>,
) -> ResolvedHistoryLocation {
    match mode {
        HistoryMode::Persistent {
            dir: Some(directory),
        } => ResolvedHistoryLocation {
            directory: Some(directory.clone()),
            source: HistoryLocationSource::Explicit,
            probe_failure: None,
        },
        HistoryMode::Persistent { dir: None } => resolve_default_directory(defaults),
        HistoryMode::Volatile => ResolvedHistoryLocation {
            directory: None,
            source: HistoryLocationSource::Volatile,
            probe_failure: None,
        },
    }
}

fn resolve_default_directory(defaults: Option<&DefaultHistoryDirs>) -> ResolvedHistoryLocation {
    let resolved = |directory: Option<PathBuf>, source| ResolvedHistoryLocation {
        directory,
        source,
        probe_failure: None,
    };
    let Some(defaults) = defaults else {
        return resolved(None, HistoryLocationSource::Default);
    };
    if defaults.current == defaults.legacy {
        return resolved(
            Some(defaults.current.clone()),
            HistoryLocationSource::Default,
        );
    }
    let selected = directory_has_history(&defaults.current).and_then(|current_has_history| {
        if current_has_history {
            return Ok(resolved(
                Some(defaults.current.clone()),
                HistoryLocationSource::Default,
            ));
        }
        Ok(if directory_has_history(&defaults.legacy)? {
            resolved(
                Some(defaults.legacy.clone()),
                HistoryLocationSource::LegacyDefault,
            )
        } else {
            resolved(
                Some(defaults.current.clone()),
                HistoryLocationSource::Default,
            )
        })
    });
    selected.unwrap_or_else(|failure| ResolvedHistoryLocation {
        directory: None,
        source: HistoryLocationSource::Default,
        probe_failure: Some(failure),
    })
}

/// Whether `directory` contains `r.db` or `shell.db`. An empty or missing
/// directory has no history; any other inspection error is reported.
fn directory_has_history(directory: &Path) -> Result<bool, HistoryProbeFailure> {
    for filename in ["r.db", "shell.db"] {
        match directory.join(filename).try_exists() {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(error) => {
                return Err(HistoryProbeFailure {
                    directory: directory.to_path_buf(),
                    message: format!("cannot check {filename}: {error}"),
                });
            }
        }
    }
    Ok(false)
}

/// The platform default history directories, independent of any explicit
/// override (`--history-dir`, `ARF_HISTORY_DIR`, or `history.mode`).
///
/// `current` is the preferred default. `legacy` is the previous default, which
/// is still used when it holds history and `current` does not, and is where
/// `arf history migrate` copies from. The two are equal wherever the default
/// location has never moved. Both are derived from environment lookup only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DefaultHistoryDirs {
    pub(crate) current: PathBuf,
    pub(crate) legacy: PathBuf,
}

/// History configuration.
#[derive(Debug, Clone)]
pub struct HistoryConfig {
    /// Maximum height (rows) for the history search menu (Ctrl+R).
    /// The actual height is the minimum of this value and the terminal height minus overhead.
    pub menu_max_height: u16,

    /// Persistent or session-only history behavior.
    pub mode: HistoryMode,
}

impl Serialize for HistoryConfig {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        #[serde(untagged)]
        enum WireMode<'a> {
            Name(&'a str),
            Persistent { dir: &'a PathBuf },
        }

        #[derive(Serialize)]
        struct WireHistory<'a> {
            menu_max_height: u16,
            mode: WireMode<'a>,
        }

        let mode = match &self.mode {
            HistoryMode::Persistent { dir: Some(dir) } => WireMode::Persistent { dir },
            HistoryMode::Persistent { dir: None } => WireMode::Name("persistent"),
            HistoryMode::Volatile => WireMode::Name("volatile"),
        };
        WireHistory {
            menu_max_height: self.menu_max_height,
            mode,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for HistoryConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawMode {
            Name(String),
            Persistent(RawPersistentMode),
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawPersistentMode {
            dir: PathBuf,
        }

        #[derive(Deserialize)]
        struct RawHistoryConfig {
            #[serde(default = "default_menu_max_height")]
            menu_max_height: u16,
            #[serde(default)]
            mode: Option<RawMode>,
            #[serde(default)]
            dir: Option<PathBuf>,
            #[serde(default)]
            disabled: Option<bool>,
        }

        let raw = RawHistoryConfig::deserialize(deserializer)?;
        let mode = match raw.mode {
            Some(RawMode::Name(mode)) => {
                if raw.dir.is_some() {
                    return Err(serde::de::Error::custom(
                        "history.dir cannot be used when history.mode is set",
                    ));
                }
                match (mode.as_str(), raw.disabled) {
                    ("persistent", _) => HistoryMode::Persistent { dir: None },
                    ("volatile", _) => HistoryMode::Volatile,
                    _ => {
                        return Err(serde::de::Error::unknown_variant(
                            &mode,
                            &["persistent", "volatile"],
                        ));
                    }
                }
            }
            Some(RawMode::Persistent(RawPersistentMode { dir })) => {
                if raw.dir.is_some() {
                    return Err(serde::de::Error::custom(
                        "history.dir cannot be used when history.mode is set",
                    ));
                }
                HistoryMode::Persistent { dir: Some(dir) }
            }
            None => match (raw.disabled, raw.dir) {
                (Some(true), _) => HistoryMode::Volatile,
                (Some(false), dir) | (None, dir) => HistoryMode::Persistent { dir },
            },
        };
        Ok(Self {
            menu_max_height: raw.menu_max_height,
            mode,
        })
    }
}

impl JsonSchema for HistoryConfig {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("HistoryConfig")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "description": "History configuration.",
            "type": "object",
            "properties": {
                "menu_max_height": {
                    "description": "Maximum height (rows) for the history search menu (Ctrl+R).",
                    "type": "integer",
                    "format": "uint16",
                    "default": 15,
                    "maximum": 65535,
                    "minimum": 0
                },
                "mode": {
                    "default": "persistent",
                    "oneOf": [
                        {
                            "type": "string",
                            "enum": ["persistent", "volatile"]
                        },
                        {
                            "type": "object",
                            "properties": {
                                "dir": { "type": "string" }
                            },
                            "required": ["dir"],
                            "additionalProperties": false
                        }
                    ]
                }
            }
        })
    }
}

fn default_menu_max_height() -> u16 {
    15
}

impl Default for HistoryConfig {
    fn default() -> Self {
        HistoryConfig {
            menu_max_height: 15,
            mode: HistoryMode::Persistent { dir: None },
        }
    }
}

#[cfg(test)]
mod location_tests {
    use super::*;
    use std::fs;

    struct Dirs {
        _root: tempfile::TempDir,
        defaults: DefaultHistoryDirs,
    }

    impl Dirs {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let defaults = DefaultHistoryDirs {
                current: root.path().join("state").join("arf").join("history"),
                legacy: root.path().join("data").join("arf").join("history"),
            };
            Self {
                _root: root,
                defaults,
            }
        }

        fn touch(directory: &Path, filename: &str) {
            fs::create_dir_all(directory).unwrap();
            fs::write(directory.join(filename), b"").unwrap();
        }

        fn resolve(&self) -> ResolvedHistoryLocation {
            resolve_history_location(&HistoryMode::Persistent { dir: None }, Some(&self.defaults))
        }
    }

    fn assert_selected(
        resolved: &ResolvedHistoryLocation,
        directory: &Path,
        source: HistoryLocationSource,
    ) {
        assert_eq!(resolved.directory(), Some(directory));
        assert_eq!(resolved.source(), source);
        assert!(resolved.probe_failure().is_none());
        // R and Shell histories always share one directory.
        assert_eq!(
            resolved.database_path("r.db").unwrap().parent(),
            resolved.database_path("shell.db").unwrap().parent()
        );
    }

    #[test]
    fn neither_directory_has_history_selects_current() {
        let dirs = Dirs::new();
        assert_selected(
            &dirs.resolve(),
            &dirs.defaults.current,
            HistoryLocationSource::Default,
        );

        // An empty legacy directory does not count as history.
        fs::create_dir_all(&dirs.defaults.legacy).unwrap();
        let resolved = dirs.resolve();
        assert_selected(
            &resolved,
            &dirs.defaults.current,
            HistoryLocationSource::Default,
        );
        assert!(resolved.legacy_default_warning().is_none());
    }

    #[test]
    fn legacy_with_either_database_selects_the_whole_legacy_directory() {
        for filename in ["r.db", "shell.db"] {
            let dirs = Dirs::new();
            Dirs::touch(&dirs.defaults.legacy, filename);
            let resolved = dirs.resolve();
            assert_selected(
                &resolved,
                &dirs.defaults.legacy,
                HistoryLocationSource::LegacyDefault,
            );
            let warning = resolved.legacy_default_warning().unwrap();
            assert!(warning.contains(&dirs.defaults.legacy.display().to_string()));
            assert!(warning.contains("arf history migrate"));
        }
    }

    #[test]
    fn current_history_wins_without_a_warning() {
        let dirs = Dirs::new();
        Dirs::touch(&dirs.defaults.current, "shell.db");
        let resolved = dirs.resolve();
        assert_selected(
            &resolved,
            &dirs.defaults.current,
            HistoryLocationSource::Default,
        );
        assert!(resolved.legacy_default_warning().is_none());

        // With history in both, current still wins and nothing is merged.
        Dirs::touch(&dirs.defaults.legacy, "r.db");
        let resolved = dirs.resolve();
        assert_selected(
            &resolved,
            &dirs.defaults.current,
            HistoryLocationSource::Default,
        );
        assert!(resolved.legacy_default_warning().is_none());
    }

    #[test]
    fn legacy_selection_gives_r_and_shell_runtimes_the_same_directory() {
        use crate::history::HistoryRuntime;
        use crate::history::artifact::HistoryKind;

        let dirs = Dirs::new();
        Dirs::touch(&dirs.defaults.legacy, "r.db");
        let resolved = dirs.resolve();
        let mode = HistoryMode::Persistent { dir: None };

        let r = HistoryRuntime::initialize_for_location(
            &mode,
            &resolved,
            "r.db",
            HistoryKind::R,
            None,
            None,
        );
        let shell = HistoryRuntime::initialize_for_location(
            &mode,
            &resolved,
            "shell.db",
            HistoryKind::Shell,
            None,
            None,
        );
        assert_eq!(r.state_name(), "persistent");
        assert_eq!(shell.state_name(), "persistent");
        assert!(dirs.defaults.legacy.join("shell.db").exists());
        assert!(!dirs.defaults.current.exists());
    }

    #[test]
    fn equal_default_directories_are_not_probed() {
        let root = tempfile::tempdir().unwrap();
        let same = root.path().join("history");
        let defaults = DefaultHistoryDirs {
            current: same.clone(),
            legacy: same.clone(),
        };
        let resolved =
            resolve_history_location(&HistoryMode::Persistent { dir: None }, Some(&defaults));
        assert_selected(&resolved, &same, HistoryLocationSource::Default);
    }

    #[test]
    fn missing_default_directories_leave_no_directory() {
        let resolved = resolve_history_location(&HistoryMode::Persistent { dir: None }, None);
        assert_eq!(resolved.directory(), None);
        assert_eq!(resolved.source(), HistoryLocationSource::Default);
        assert!(resolved.probe_failure().is_none());
    }

    #[test]
    fn explicit_directory_wins_without_probing_or_warning() {
        let dirs = Dirs::new();
        Dirs::touch(&dirs.defaults.legacy, "r.db");
        let explicit = dirs.defaults.current.join("elsewhere");
        let resolved = resolve_history_location(
            &HistoryMode::Persistent {
                dir: Some(explicit.clone()),
            },
            Some(&dirs.defaults),
        );
        assert_selected(&resolved, &explicit, HistoryLocationSource::Explicit);

        // Explicitly naming the legacy path is still explicit.
        let resolved = resolve_history_location(
            &HistoryMode::Persistent {
                dir: Some(dirs.defaults.legacy.clone()),
            },
            Some(&dirs.defaults),
        );
        assert_selected(
            &resolved,
            &dirs.defaults.legacy,
            HistoryLocationSource::Explicit,
        );
        assert!(resolved.legacy_default_warning().is_none());
    }

    #[test]
    fn volatile_never_looks_at_persistent_paths() {
        let dirs = Dirs::new();
        Dirs::touch(&dirs.defaults.legacy, "r.db");
        let resolved = resolve_history_location(&HistoryMode::Volatile, Some(&dirs.defaults));
        assert_eq!(resolved.directory(), None);
        assert_eq!(resolved.database_path("r.db"), None);
        assert_eq!(resolved.source(), HistoryLocationSource::Volatile);
        assert!(resolved.legacy_default_warning().is_none());
    }

    #[cfg(unix)]
    mod unix_probe {
        use super::*;
        use std::os::unix::fs::PermissionsExt;

        /// Restores directory permissions so the tempdir can be removed.
        struct Locked(PathBuf);

        impl Locked {
            /// Returns `None` when the permissions are not enforced (running as
            /// root), in which case the caller skips the test.
            fn new(directory: &Path) -> Option<Self> {
                fs::set_permissions(directory, fs::Permissions::from_mode(0o000)).unwrap();
                let locked = Self(directory.to_path_buf());
                fs::read_dir(directory).is_err().then_some(locked)
            }
        }

        impl Drop for Locked {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o700));
            }
        }

        #[test]
        fn unreadable_legacy_directory_is_a_failure_not_absence() {
            let dirs = Dirs::new();
            fs::create_dir_all(&dirs.defaults.current).unwrap();
            fs::create_dir_all(&dirs.defaults.legacy).unwrap();
            let Some(_locked) = Locked::new(&dirs.defaults.legacy) else {
                return;
            };

            let resolved = dirs.resolve();
            assert_eq!(resolved.directory(), None);
            assert_eq!(resolved.database_path("r.db"), None);
            let failure = resolved.probe_failure().expect("probe error is reported");
            assert_eq!(failure.directory(), dirs.defaults.legacy);
            assert!(
                failure
                    .to_string()
                    .contains(&dirs.defaults.legacy.display().to_string())
            );
            assert!(resolved.legacy_default_warning().is_none());
        }

        #[test]
        fn probe_failure_becomes_a_volatile_fallback_naming_the_directory() {
            use crate::history::HistoryRuntime;
            use crate::history::artifact::HistoryKind;

            let dirs = Dirs::new();
            fs::create_dir_all(&dirs.defaults.current).unwrap();
            fs::create_dir_all(&dirs.defaults.legacy).unwrap();
            let Some(_locked) = Locked::new(&dirs.defaults.legacy) else {
                return;
            };
            let resolved = dirs.resolve();

            for (filename, kind) in [("r.db", HistoryKind::R), ("shell.db", HistoryKind::Shell)] {
                let runtime = HistoryRuntime::initialize_for_location(
                    &HistoryMode::Persistent { dir: None },
                    &resolved,
                    filename,
                    kind,
                    None,
                    None,
                );
                assert_eq!(runtime.state_name(), "volatile");
                assert_eq!(runtime.reason_name(), Some("fallback"));
                assert_eq!(
                    runtime.requested_path(),
                    Some(dirs.defaults.legacy.as_path())
                );
                let warning = runtime.startup_warning().unwrap();
                assert!(warning.contains(&dirs.defaults.legacy.display().to_string()));
            }
            assert!(!dirs.defaults.current.join("r.db").exists());
        }

        #[test]
        fn unreadable_current_directory_is_a_failure_not_absence() {
            let dirs = Dirs::new();
            Dirs::touch(&dirs.defaults.legacy, "r.db");
            fs::create_dir_all(&dirs.defaults.current).unwrap();
            let Some(_locked) = Locked::new(&dirs.defaults.current) else {
                return;
            };

            let resolved = dirs.resolve();
            assert_eq!(resolved.directory(), None);
            assert_eq!(
                resolved.probe_failure().unwrap().directory(),
                dirs.defaults.current
            );
        }

        #[test]
        fn unreadable_legacy_is_ignored_when_current_has_history() {
            let dirs = Dirs::new();
            Dirs::touch(&dirs.defaults.current, "r.db");
            fs::create_dir_all(&dirs.defaults.legacy).unwrap();
            let Some(_locked) = Locked::new(&dirs.defaults.legacy) else {
                return;
            };

            assert_selected(
                &dirs.resolve(),
                &dirs.defaults.current,
                HistoryLocationSource::Default,
            );
        }
    }
}
