use super::*;
use crate::history::HistoryRuntime;
use crate::history::HistoryStore;
use crate::history::artifact::HistoryKind;
use crate::history::import::migrate::migrate_history;
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

    /// Create a real, empty arf history database of `kind` in `directory`.
    fn database(directory: &Path, kind: HistoryKind) {
        let filename = match kind {
            HistoryKind::R => "r.db",
            HistoryKind::Shell => "shell.db",
        };
        HistoryStore::open(directory.join(filename), kind, None, None).unwrap();
    }

    /// Copy the legacy history into the current directory the way
    /// `arf history migrate` does.
    fn migrate(&self) {
        migrate_history(&self.legacy, &self.current, false).unwrap();
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
    Dirs::database(&dirs.current, HistoryKind::Shell);
    Dirs::database(&dirs.legacy, HistoryKind::R);
    let resolved = dirs.resolve();
    assert_selected(&resolved, &dirs.current, HistoryLocationSource::Default);
    assert!(resolved.legacy_default_warning().is_none());
    let warning = resolved.startup_warning().unwrap();
    assert!(warning.contains("R history"));
    assert!(!warning.contains("shell history"));
    assert!(warning.contains(&dirs.legacy.join("r.db").display().to_string()));
    assert!(warning.contains(&dirs.current.display().to_string()));
    assert!(warning.contains("has not been copied"));
    assert!(warning.contains("arf history migrate"));

    // Current has only R history; legacy has shell history.
    let dirs = Dirs::new();
    Dirs::database(&dirs.current, HistoryKind::R);
    Dirs::database(&dirs.legacy, HistoryKind::Shell);
    let warning = dirs.resolve().startup_warning().unwrap();
    assert!(warning.contains("shell history"));
    assert!(!warning.contains("R history"));
    assert!(warning.contains(&dirs.legacy.join("shell.db").display().to_string()));
}

#[test]
fn warning_survives_startup_creating_the_missing_database() {
    let dirs = Dirs::new();
    Dirs::database(&dirs.current, HistoryKind::Shell);
    Dirs::database(&dirs.legacy, HistoryKind::R);
    let resolved = dirs.resolve();
    assert!(resolved.startup_warning().is_some());

    // A session start opens both databases in the selected directory, which
    // creates the empty current r.db.
    let mode = HistoryMode::Persistent { dir: None };
    for (filename, kind) in [("r.db", HistoryKind::R), ("shell.db", HistoryKind::Shell)] {
        let runtime =
            HistoryRuntime::initialize_for_location(&mode, &resolved, filename, kind, None, None);
        assert_eq!(runtime.state_name(), "persistent");
    }
    assert!(dirs.current.join("r.db").exists());

    let warning = dirs
        .resolve()
        .startup_warning()
        .expect("the legacy R history is still uncopied");
    assert!(warning.contains(&dirs.legacy.join("r.db").display().to_string()));
}

#[test]
fn migration_silences_the_warning_while_the_legacy_directory_is_kept() {
    let dirs = Dirs::new();
    Dirs::database(&dirs.current, HistoryKind::Shell);
    Dirs::database(&dirs.legacy, HistoryKind::R);
    Dirs::database(&dirs.legacy, HistoryKind::Shell);
    assert!(dirs.resolve().startup_warning().is_some());

    dirs.migrate();

    let resolved = dirs.resolve();
    assert_selected(&resolved, &dirs.current, HistoryLocationSource::Default);
    assert!(resolved.startup_warning().is_none());
    assert!(dirs.legacy.join("r.db").exists());
    assert!(dirs.legacy.join("shell.db").exists());
}

#[test]
fn migration_from_another_legacy_directory_does_not_silence_the_warning() {
    let dirs = Dirs::new();
    let other = dirs._root.path().join("other");
    Dirs::database(&other, HistoryKind::R);
    Dirs::database(&dirs.legacy, HistoryKind::R);
    migrate_history(&other, &dirs.current, false).unwrap();

    let warning = dirs
        .resolve()
        .startup_warning()
        .expect("the record names a different legacy directory");
    assert!(warning.contains(&dirs.legacy.join("r.db").display().to_string()));

    dirs.migrate();
    assert!(dirs.resolve().startup_warning().is_none());
}

#[test]
fn legacy_history_of_a_kind_created_after_the_migration_is_reported() {
    let dirs = Dirs::new();
    Dirs::database(&dirs.legacy, HistoryKind::R);
    dirs.migrate();
    assert!(dirs.resolve().startup_warning().is_none());

    Dirs::database(&dirs.legacy, HistoryKind::Shell);
    let warning = dirs.resolve().startup_warning().unwrap();
    assert!(warning.contains("shell history"));
    assert!(!warning.contains("R history"));
    assert!(warning.contains(&dirs.legacy.join("shell.db").display().to_string()));
}

#[test]
fn legacy_history_of_a_kind_recreated_after_a_later_migration_is_reported() {
    let dirs = Dirs::new();
    Dirs::database(&dirs.legacy, HistoryKind::R);
    Dirs::database(&dirs.legacy, HistoryKind::Shell);
    dirs.migrate();
    std::fs::remove_file(dirs.legacy.join("shell.db")).unwrap();
    dirs.migrate();

    Dirs::database(&dirs.legacy, HistoryKind::Shell);
    let warning = dirs.resolve().startup_warning().unwrap();
    assert!(warning.contains(&dirs.legacy.join("shell.db").display().to_string()));
    assert!(!warning.contains("R history"));
}

#[test]
fn legacy_history_recreated_after_a_migration_that_found_none_is_reported() {
    let dirs = Dirs::new();
    Dirs::database(&dirs.legacy, HistoryKind::R);
    Dirs::database(&dirs.legacy, HistoryKind::Shell);
    dirs.migrate();
    std::fs::remove_file(dirs.legacy.join("r.db")).unwrap();
    std::fs::remove_file(dirs.legacy.join("shell.db")).unwrap();
    dirs.migrate();

    Dirs::database(&dirs.legacy, HistoryKind::R);
    let warning = dirs.resolve().startup_warning().unwrap();
    assert!(warning.contains(&dirs.legacy.join("r.db").display().to_string()));
}

#[test]
fn same_kinds_in_both_directories_warn_until_migrated() {
    let dirs = Dirs::new();
    for directory in [&dirs.current, &dirs.legacy] {
        Dirs::database(directory, HistoryKind::R);
        Dirs::database(directory, HistoryKind::Shell);
    }
    let warning = dirs.resolve().startup_warning().unwrap();
    assert!(warning.contains("R history"));
    assert!(warning.contains("shell history"));

    dirs.migrate();
    assert!(dirs.resolve().startup_warning().is_none());
}

#[test]
fn no_legacy_databases_means_no_warning() {
    // A current-only user, with or without an empty legacy directory.
    let dirs = Dirs::new();
    Dirs::database(&dirs.current, HistoryKind::Shell);
    assert!(dirs.resolve().startup_warning().is_none());
    fs::create_dir_all(&dirs.legacy).unwrap();
    assert!(dirs.resolve().startup_warning().is_none());
}

#[test]
fn unreadable_current_database_metadata_is_reported_not_ignored() {
    let dirs = Dirs::new();
    Dirs::touch(&dirs.current, "r.db");
    fs::write(dirs.current.join("r.db"), b"this is not a sqlite database").unwrap();
    Dirs::database(&dirs.legacy, HistoryKind::R);

    let resolved = dirs.resolve();
    // The session stays persistent in the current directory.
    assert_selected(&resolved, &dirs.current, HistoryLocationSource::Default);
    let warning = resolved.startup_warning().unwrap();
    assert!(warning.contains("could not check"));
    assert!(warning.contains(&dirs.current.join("r.db").display().to_string()));
    assert!(!warning.contains("has not been copied to the current default location, so"));
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
    fn unreadable_legacy_is_reported_without_failing_the_session_location() {
        let dirs = Dirs::new();
        Dirs::database(&dirs.current, HistoryKind::R);
        fs::create_dir_all(&dirs.legacy).unwrap();
        let Some(_locked) = Locked::new(&dirs.legacy) else {
            return;
        };
        let resolved = dirs.resolve();
        assert_selected(&resolved, &dirs.current, HistoryLocationSource::Default);
        let warning = resolved.startup_warning().unwrap();
        assert!(warning.contains("could not check"));
        assert!(warning.contains(&dirs.legacy.display().to_string()));
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
