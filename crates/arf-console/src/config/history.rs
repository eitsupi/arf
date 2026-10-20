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
    /// The current default directory, kept only when the legacy default was
    /// selected so the startup warning can name both.
    current_default: Option<PathBuf>,
    /// Legacy databases whose kind is missing from the selected current
    /// default directory, so their history is not in use.
    legacy_only: Vec<LegacyOnlyDatabase>,
}

/// A legacy database that has no counterpart in the current default directory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LegacyOnlyDatabase {
    label: &'static str,
    path: PathBuf,
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

    /// The startup warning for this location, if any: either the legacy
    /// directory was selected, or the current default is missing a kind of
    /// history that only the legacy directory has.
    pub(crate) fn startup_warning(&self) -> Option<String> {
        self.legacy_default_warning()
            .or_else(|| self.split_history_warning())
    }

    /// Startup warning shown only when the legacy default directory was
    /// selected automatically. It is a multi-line message naming both
    /// directories and the way to switch to the current default.
    pub(crate) fn legacy_default_warning(&self) -> Option<String> {
        if self.source != HistoryLocationSource::LegacyDefault {
            return None;
        }
        let legacy = self.directory.as_deref()?;
        let current = self.current_default.as_deref()?;
        Some(format!(
            "arf is using history from the legacy default location.\n  \
             Legacy location:  {}\n  \
             Current default:  {}\n\
             The current default location has no history yet, so arf keeps reading and \
             writing the legacy location.\n\
             Run `arf history migrate` to copy your history to the current default \
             location; arf uses it from then on. The legacy location is left unchanged.",
            legacy.display(),
            current.display()
        ))
    }

    /// Warning shown when the current default was selected but the legacy
    /// directory holds a kind of history (R or shell) that the current one
    /// lacks. Selection stays per directory, so that history is not recalled
    /// until it is copied.
    fn split_history_warning(&self) -> Option<String> {
        if self.legacy_only.is_empty() {
            return None;
        }
        let current = self.directory.as_deref()?;
        let mut message = String::from(
            "arf found history in the legacy default location that is missing from the \
             current default location, so it is not used:",
        );
        for database in &self.legacy_only {
            message.push_str(&format!(
                "\n  {:<22}{}",
                format!("{}:", database.label),
                database.path.display()
            ));
        }
        message.push_str(&format!(
            "\n  {:<22}{}\n\
             Run `arf history migrate` to copy it (duplicates are skipped); the legacy \
             location is left unchanged.",
            "Current default:",
            current.display()
        ));
        Some(message)
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
            current_default: None,
            legacy_only: Vec::new(),
        },
        HistoryMode::Persistent { dir: None } => resolve_default_directory(defaults),
        HistoryMode::Volatile => ResolvedHistoryLocation {
            directory: None,
            source: HistoryLocationSource::Volatile,
            probe_failure: None,
            current_default: None,
            legacy_only: Vec::new(),
        },
    }
}

fn resolve_default_directory(defaults: Option<&DefaultHistoryDirs>) -> ResolvedHistoryLocation {
    let resolved = |directory: Option<PathBuf>, source| ResolvedHistoryLocation {
        directory,
        source,
        probe_failure: None,
        current_default: None,
        legacy_only: Vec::new(),
    };
    let Some(defaults) = defaults else {
        return resolved(None, HistoryLocationSource::Default);
    };
    let Some(legacy) = defaults
        .legacy
        .as_ref()
        .filter(|legacy| **legacy != defaults.current)
    else {
        return resolved(
            Some(defaults.current.clone()),
            HistoryLocationSource::Default,
        );
    };
    let selected = probe_databases(&defaults.current).and_then(|current| {
        if current.any() {
            let mut location = resolved(
                Some(defaults.current.clone()),
                HistoryLocationSource::Default,
            );
            // Only a partially populated current directory can hide legacy
            // history. This check is best effort: legacy errors are ignored
            // because the current directory already has history.
            if !(current.r && current.shell)
                && let Ok(found) = probe_databases(legacy)
            {
                location.legacy_only = [
                    ("Legacy R history", "r.db", found.r && !current.r),
                    (
                        "Legacy shell history",
                        "shell.db",
                        found.shell && !current.shell,
                    ),
                ]
                .into_iter()
                .filter(|(_, _, missing)| *missing)
                .map(|(label, filename, _)| LegacyOnlyDatabase {
                    label,
                    path: legacy.join(filename),
                })
                .collect();
            }
            return Ok(location);
        }
        Ok(if probe_databases(legacy)?.any() {
            ResolvedHistoryLocation {
                current_default: Some(defaults.current.clone()),
                ..resolved(Some(legacy.clone()), HistoryLocationSource::LegacyDefault)
            }
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
        current_default: None,
        legacy_only: Vec::new(),
    })
}

/// Which history databases a directory contains.
struct DatabasePresence {
    r: bool,
    shell: bool,
}

impl DatabasePresence {
    fn any(&self) -> bool {
        self.r || self.shell
    }
}

/// Check for `r.db` and `shell.db` in `directory`. A missing directory simply
/// has neither; any other inspection error is reported rather than treated as
/// absence.
fn probe_databases(directory: &Path) -> Result<DatabasePresence, HistoryProbeFailure> {
    let exists = |filename: &str| {
        directory
            .join(filename)
            .try_exists()
            .map_err(|error| HistoryProbeFailure {
                directory: directory.to_path_buf(),
                message: format!("cannot check {filename}: {error}"),
            })
    };
    Ok(DatabasePresence {
        r: exists("r.db")?,
        shell: exists("shell.db")?,
    })
}

/// The platform default history directories, independent of any explicit
/// override (`--history-dir`, `ARF_HISTORY_DIR`, or `history.mode`).
///
/// `current` is the preferred default. `legacy` is the previous default, which
/// is still used when it holds history and `current` does not, and is where
/// `arf history migrate` copies from. The two are equal wherever the default
/// location has never moved. `legacy` is `None` when the previous default
/// cannot be determined (distinct from "same as current"); `current` is then
/// used as is. Both are derived from environment lookup only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DefaultHistoryDirs {
    pub(crate) current: PathBuf,
    pub(crate) legacy: Option<PathBuf>,
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
        current: PathBuf,
        legacy: PathBuf,
        defaults: DefaultHistoryDirs,
    }

    impl Dirs {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            let current = root.path().join("state").join("arf").join("history");
            let legacy = root.path().join("data").join("arf").join("history");
            let defaults = DefaultHistoryDirs {
                current: current.clone(),
                legacy: Some(legacy.clone()),
            };
            Self {
                _root: root,
                current,
                legacy,
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
            &dirs.current,
            HistoryLocationSource::Default,
        );

        // An empty legacy directory does not count as history.
        fs::create_dir_all(&dirs.legacy).unwrap();
        let resolved = dirs.resolve();
        assert_selected(&resolved, &dirs.current, HistoryLocationSource::Default);
        assert!(resolved.legacy_default_warning().is_none());
    }

    #[test]
    fn legacy_with_either_database_selects_the_whole_legacy_directory() {
        for filename in ["r.db", "shell.db"] {
            let dirs = Dirs::new();
            Dirs::touch(&dirs.legacy, filename);
            let resolved = dirs.resolve();
            assert_selected(
                &resolved,
                &dirs.legacy,
                HistoryLocationSource::LegacyDefault,
            );
            let warning = resolved.legacy_default_warning().unwrap();
            assert!(warning.contains(&dirs.legacy.display().to_string()));
            assert!(warning.contains(&dirs.current.display().to_string()));
            assert!(warning.contains("arf history migrate"));
            assert!(warning.contains("copy"));
        }
    }

    #[test]
    fn current_history_wins_without_a_warning() {
        let dirs = Dirs::new();
        Dirs::touch(&dirs.current, "shell.db");
        let resolved = dirs.resolve();
        assert_selected(&resolved, &dirs.current, HistoryLocationSource::Default);
        assert!(resolved.legacy_default_warning().is_none());

        // With history in both, current still wins and nothing is merged.
        Dirs::touch(&dirs.legacy, "r.db");
        let resolved = dirs.resolve();
        assert_selected(&resolved, &dirs.current, HistoryLocationSource::Default);
        assert!(resolved.legacy_default_warning().is_none());
    }

    #[test]
    fn current_missing_a_kind_that_legacy_has_warns_naming_that_kind() {
        // Current has only shell history; legacy has R history.
        let dirs = Dirs::new();
        Dirs::touch(&dirs.current, "shell.db");
        Dirs::touch(&dirs.legacy, "r.db");
        let resolved = dirs.resolve();
        assert_selected(&resolved, &dirs.current, HistoryLocationSource::Default);
        assert!(resolved.legacy_default_warning().is_none());
        let warning = resolved.startup_warning().unwrap();
        assert!(warning.contains("R history"));
        assert!(!warning.contains("shell history"));
        assert!(warning.contains(&dirs.legacy.join("r.db").display().to_string()));
        assert!(warning.contains(&dirs.current.display().to_string()));
        assert!(warning.contains("arf history migrate"));

        // Current has only R history; legacy has shell history.
        let dirs = Dirs::new();
        Dirs::touch(&dirs.current, "r.db");
        Dirs::touch(&dirs.legacy, "shell.db");
        let warning = dirs.resolve().startup_warning().unwrap();
        assert!(warning.contains("shell history"));
        assert!(!warning.contains("R history"));
        assert!(warning.contains(&dirs.legacy.join("shell.db").display().to_string()));
    }

    #[test]
    fn same_kinds_in_both_directories_or_a_complete_current_do_not_warn() {
        // Normal after a migration: legacy is kept alongside current.
        let dirs = Dirs::new();
        for directory in [&dirs.current, &dirs.legacy] {
            Dirs::touch(directory, "r.db");
            Dirs::touch(directory, "shell.db");
        }
        assert!(dirs.resolve().startup_warning().is_none());

        // Same single kind on both sides.
        let dirs = Dirs::new();
        Dirs::touch(&dirs.current, "r.db");
        Dirs::touch(&dirs.legacy, "r.db");
        assert!(dirs.resolve().startup_warning().is_none());

        // A complete current directory never reports legacy leftovers.
        let dirs = Dirs::new();
        Dirs::touch(&dirs.current, "r.db");
        Dirs::touch(&dirs.current, "shell.db");
        Dirs::touch(&dirs.legacy, "r.db");
        assert!(dirs.resolve().startup_warning().is_none());
    }

    #[test]
    fn legacy_selection_gives_r_and_shell_runtimes_the_same_directory() {
        use crate::history::HistoryRuntime;
        use crate::history::artifact::HistoryKind;

        let dirs = Dirs::new();
        Dirs::touch(&dirs.legacy, "r.db");
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
        assert!(dirs.legacy.join("shell.db").exists());
        assert!(!dirs.current.exists());
    }

    #[test]
    fn equal_default_directories_are_not_probed() {
        let root = tempfile::tempdir().unwrap();
        let same = root.path().join("history");
        let defaults = DefaultHistoryDirs {
            current: same.clone(),
            legacy: Some(same.clone()),
        };
        let resolved =
            resolve_history_location(&HistoryMode::Persistent { dir: None }, Some(&defaults));
        assert_selected(&resolved, &same, HistoryLocationSource::Default);
    }

    #[test]
    fn unknown_legacy_directory_selects_current_without_probing_legacy() {
        let root = tempfile::tempdir().unwrap();
        let current = root.path().join("state").join("arf").join("history");
        let defaults = DefaultHistoryDirs {
            current: current.clone(),
            legacy: None,
        };
        // With and without history in the current directory.
        for populate in [false, true] {
            if populate {
                fs::create_dir_all(&current).unwrap();
                fs::write(current.join("shell.db"), b"").unwrap();
            }
            let resolved =
                resolve_history_location(&HistoryMode::Persistent { dir: None }, Some(&defaults));
            assert_selected(&resolved, &current, HistoryLocationSource::Default);
            assert!(resolved.startup_warning().is_none());
        }
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
        Dirs::touch(&dirs.legacy, "r.db");
        let explicit = dirs.current.join("elsewhere");
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
                dir: Some(dirs.legacy.clone()),
            },
            Some(&dirs.defaults),
        );
        assert_selected(&resolved, &dirs.legacy, HistoryLocationSource::Explicit);
        assert!(resolved.legacy_default_warning().is_none());
    }

    #[test]
    fn volatile_never_looks_at_persistent_paths() {
        let dirs = Dirs::new();
        Dirs::touch(&dirs.legacy, "r.db");
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
            fs::create_dir_all(&dirs.current).unwrap();
            fs::create_dir_all(&dirs.legacy).unwrap();
            let Some(_locked) = Locked::new(&dirs.legacy) else {
                return;
            };

            let resolved = dirs.resolve();
            assert_eq!(resolved.directory(), None);
            assert_eq!(resolved.database_path("r.db"), None);
            let failure = resolved.probe_failure().expect("probe error is reported");
            assert_eq!(failure.directory(), dirs.legacy);
            assert!(
                failure
                    .to_string()
                    .contains(&dirs.legacy.display().to_string())
            );
            assert!(resolved.legacy_default_warning().is_none());
        }

        #[test]
        fn probe_failure_becomes_a_volatile_fallback_naming_the_directory() {
            use crate::history::HistoryRuntime;
            use crate::history::artifact::HistoryKind;

            let dirs = Dirs::new();
            fs::create_dir_all(&dirs.current).unwrap();
            fs::create_dir_all(&dirs.legacy).unwrap();
            let Some(_locked) = Locked::new(&dirs.legacy) else {
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
                assert_eq!(runtime.requested_path(), Some(dirs.legacy.as_path()));
                let warning = runtime.startup_warning().unwrap();
                assert!(warning.contains(&dirs.legacy.display().to_string()));
            }
            assert!(!dirs.current.join("r.db").exists());
        }

        #[test]
        fn unreadable_legacy_does_not_fail_split_history_detection() {
            let dirs = Dirs::new();
            Dirs::touch(&dirs.current, "r.db");
            fs::create_dir_all(&dirs.legacy).unwrap();
            let Some(_locked) = Locked::new(&dirs.legacy) else {
                return;
            };
            let resolved = dirs.resolve();
            assert_selected(&resolved, &dirs.current, HistoryLocationSource::Default);
            assert!(resolved.startup_warning().is_none());
        }

        #[test]
        fn unreadable_current_directory_is_a_failure_not_absence() {
            let dirs = Dirs::new();
            Dirs::touch(&dirs.legacy, "r.db");
            fs::create_dir_all(&dirs.current).unwrap();
            let Some(_locked) = Locked::new(&dirs.current) else {
                return;
            };

            let resolved = dirs.resolve();
            assert_eq!(resolved.directory(), None);
            assert_eq!(resolved.probe_failure().unwrap().directory(), dirs.current);
        }

        #[test]
        fn unreadable_legacy_is_ignored_when_current_has_history() {
            let dirs = Dirs::new();
            Dirs::touch(&dirs.current, "r.db");
            fs::create_dir_all(&dirs.legacy).unwrap();
            let Some(_locked) = Locked::new(&dirs.legacy) else {
                return;
            };

            assert_selected(
                &dirs.resolve(),
                &dirs.current,
                HistoryLocationSource::Default,
            );
        }
    }
}
