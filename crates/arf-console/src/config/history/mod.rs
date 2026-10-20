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
    uncopied: Vec<UncopiedLegacyDatabase>,
    /// Problems that kept the check above from completing, already phrased
    /// for display. They never change which directory is used.
    check_problems: Vec<String>,
}

/// A legacy database whose history has not been copied to the current default
/// directory: the current database of that kind is missing or has no record of
/// a completed migration.
#[derive(Debug, Clone, PartialEq, Eq)]
struct UncopiedLegacyDatabase {
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
    /// directory was selected, or the current default was selected while
    /// legacy history has not been copied into it.
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
    /// directory holds a kind of history (R or shell) that has not been copied
    /// into it, or when that could not be checked. Selection stays per
    /// directory, so uncopied history is not recalled until it is copied.
    fn split_history_warning(&self) -> Option<String> {
        let current = self.directory.as_deref()?;
        let mut paragraphs = Vec::new();
        if !self.uncopied.is_empty() {
            let mut message = String::from(
                "arf found history in the legacy default location that has not been copied \
                 to the current default location, so it is not used:",
            );
            for database in &self.uncopied {
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
            paragraphs.push(message);
        }
        for problem in &self.check_problems {
            paragraphs.push(format!(
                "arf could not check the legacy default location for history that has not been \
                 copied to the current default location: {problem}"
            ));
        }
        (!paragraphs.is_empty()).then(|| paragraphs.join("\n"))
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
            uncopied: Vec::new(),
            check_problems: Vec::new(),
        },
        HistoryMode::Persistent { dir: None } => resolve_default_directory(defaults),
        HistoryMode::Volatile => ResolvedHistoryLocation {
            directory: None,
            source: HistoryLocationSource::Volatile,
            probe_failure: None,
            current_default: None,
            uncopied: Vec::new(),
            check_problems: Vec::new(),
        },
    }
}

fn resolve_default_directory(defaults: Option<&DefaultHistoryDirs>) -> ResolvedHistoryLocation {
    let resolved = |directory: Option<PathBuf>, source| ResolvedHistoryLocation {
        directory,
        source,
        probe_failure: None,
        current_default: None,
        uncopied: Vec::new(),
        check_problems: Vec::new(),
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
            // The current directory already has history, so the legacy check
            // is best effort: problems are reported, never fatal.
            (location.uncopied, location.check_problems) =
                check_uncopied_legacy(legacy, &defaults.current, &current);
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
        uncopied: Vec::new(),
        check_problems: Vec::new(),
    })
}

/// Find legacy databases whose history is not in the current directory, and
/// the problems that prevented checking some of them.
///
/// A legacy database counts as uncopied when the current database of the same
/// kind is missing or lacks the record a completed `arf history migrate` of
/// this legacy directory leaves. Kinds with no legacy database are never
/// inspected further, so a directory without legacy history costs only the
/// existence checks.
fn check_uncopied_legacy(
    legacy: &Path,
    current_directory: &Path,
    current: &DatabasePresence,
) -> (Vec<UncopiedLegacyDatabase>, Vec<String>) {
    let mut uncopied = Vec::new();
    let mut problems = Vec::new();
    for (label, filename, current_has_kind) in [
        ("Legacy R history", "r.db", current.r),
        ("Legacy shell history", "shell.db", current.shell),
    ] {
        let legacy_database = legacy.join(filename);
        match legacy_database.try_exists() {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) => {
                problems.push(format!(
                    "cannot check {}: {error}",
                    legacy_database.display()
                ));
                continue;
            }
        }
        let copied = if current_has_kind {
            let current_database = current_directory.join(filename);
            match crate::history::artifact::legacy_migration_recorded(&current_database, legacy) {
                Ok(recorded) => recorded,
                Err(error) => {
                    problems.push(format!(
                        "cannot read the migration status of {}: {error:#}",
                        current_database.display()
                    ));
                    continue;
                }
            }
        } else {
            false
        };
        if !copied {
            uncopied.push(UncopiedLegacyDatabase {
                label,
                path: legacy_database,
            });
        }
    }
    (uncopied, problems)
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
mod location_tests;
