//! Versioned metadata for arf-owned history database artifacts.

use anyhow::{Context, Result, bail};
use rusqlite::Connection;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

const METADATA_TABLE: &str = "arf_metadata";
const FORMAT_VERSION: &str = "1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryKind {
    R,
    Shell,
}

impl HistoryKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::R => "r",
            Self::Shell => "shell",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryArtifact {
    Legacy,
    History(HistoryKind),
    Export,
}

pub(crate) fn read_artifact(connection: &Connection) -> Result<HistoryArtifact> {
    if !metadata_table_exists(connection)? {
        return Ok(HistoryArtifact::Legacy);
    }

    let mut statement = connection
        .prepare("SELECT key, value FROM arf_metadata")
        .context("failed to read arf artifact metadata")?;
    let values = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .context("failed to read arf artifact metadata")?
        .collect::<rusqlite::Result<HashMap<_, _>>>()
        .context("invalid arf artifact metadata rows")?;

    let required = |key: &str| -> Result<&str> {
        values
            .get(key)
            .map(String::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("arf artifact metadata is missing required key '{key}'"))
    };

    let artifact = required("artifact")?;
    let version_text = required("format_version")?;
    let version = version_text.parse::<u32>().with_context(|| {
        format!("invalid arf artifact format_version '{version_text}': expected an integer")
    })?;
    let supported_version = FORMAT_VERSION
        .parse::<u32>()
        .expect("the supported artifact version is a valid integer");
    if version != supported_version {
        bail!(
            "unsupported arf artifact format version {version} (supported version: {supported_version})"
        );
    }
    required("created_by_version")?;

    match artifact {
        "history" => {
            let kind = match required("history_kind")? {
                "r" => HistoryKind::R,
                "shell" => HistoryKind::Shell,
                kind => bail!("invalid arf history_kind '{kind}': expected 'r' or 'shell'"),
            };
            Ok(HistoryArtifact::History(kind))
        }
        "history-export" => Ok(HistoryArtifact::Export),
        value => bail!("unsupported arf artifact type '{value}'"),
    }
}

pub(crate) fn read_artifact_from_path(path: &Path) -> Result<HistoryArtifact> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| {
            format!(
                "failed to open history artifact metadata from {}",
                path.display()
            )
        })?;
    read_artifact(&connection)
}

/// Validate a database as the expected single-history artifact.
/// Metadata-less legacy databases remain accepted for compatibility.
pub(crate) fn validate_history_artifact(
    connection: &Connection,
    expected_kind: HistoryKind,
) -> Result<()> {
    match read_artifact(connection)? {
        HistoryArtifact::Legacy => Ok(()),
        HistoryArtifact::History(actual_kind) if actual_kind == expected_kind => Ok(()),
        HistoryArtifact::History(actual_kind) => bail!(
            "history database kind mismatch: expected '{}', metadata says '{}'",
            expected_kind.as_str(),
            actual_kind.as_str()
        ),
        HistoryArtifact::Export => {
            bail!("cannot open a unified history export as a single history database")
        }
    }
}

/// Validate an existing history artifact or publish a fully prepared new one.
/// The target never becomes visible before its complete arf metadata exists.
pub(crate) fn prepare_history_path(path: &Path, kind: HistoryKind) -> Result<()> {
    if path
        .try_exists()
        .with_context(|| format!("failed to inspect history database path {}", path.display()))?
    {
        return validate_history_path(path, kind);
    }

    let parent = history_parent(path);
    fs::create_dir_all(parent).with_context(|| {
        format!(
            "failed to create history database directory {}",
            parent.display()
        )
    })?;

    let staged = stage_history_database(parent, kind)?;
    publish_staged_history_database(staged, path, kind)
}

fn history_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

fn stage_history_database(parent: &Path, kind: HistoryKind) -> Result<tempfile::NamedTempFile> {
    let staged = tempfile::Builder::new()
        .prefix(".arf-history-")
        .tempfile_in(parent)
        .with_context(|| format!("failed to stage history database in {}", parent.display()))?;

    let mut connection = Connection::open(staged.path())
        .with_context(|| "failed to open staged history database".to_string())?;
    initialize_history_metadata(&mut connection, kind)
        .context("failed to initialize staged history artifact metadata")?;

    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))
        .context("failed to enable WAL mode for staged history database")?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        bail!("staged history database refused WAL mode (reported '{journal_mode}')");
    }

    connection
        .close()
        .map_err(|(_, error)| error)
        .context("failed to close staged history database")?;
    Ok(staged)
}

fn publish_staged_history_database(
    staged: tempfile::NamedTempFile,
    target: &Path,
    kind: HistoryKind,
) -> Result<()> {
    match staged.persist_noclobber(target) {
        Ok(file) => {
            drop(file);
            Ok(())
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            drop(error.file);
            validate_history_path(target, kind)
        }
        Err(error) => Err(error)
            .with_context(|| format!("failed to publish history database at {}", target.display())),
    }
}

fn validate_history_path(path: &Path, kind: HistoryKind) -> Result<()> {
    let connection = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("failed to open history database {}", path.display()))?;
    validate_history_artifact(&connection, kind)
}

pub(crate) fn initialize_history_metadata(
    connection: &mut Connection,
    kind: HistoryKind,
) -> Result<()> {
    let transaction = connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .context("failed to start history artifact metadata transaction")?;
    let table_exists = metadata_table_exists(&transaction)?;
    if table_exists {
        let artifact = read_artifact(&transaction)?;
        match artifact {
            HistoryArtifact::History(existing_kind) if existing_kind == kind => {}
            HistoryArtifact::History(existing_kind) => bail!(
                "history database kind mismatch: requested '{}', metadata says '{}'",
                kind.as_str(),
                existing_kind.as_str()
            ),
            HistoryArtifact::Export => {
                bail!("cannot initialize a history database from a unified history export")
            }
            HistoryArtifact::Legacy => {
                bail!("arf artifact metadata table disappeared during initialization")
            }
        }
    } else {
        write_metadata_in_transaction(
            &transaction,
            &[
                ("artifact", "history"),
                ("format_version", FORMAT_VERSION),
                ("history_kind", kind.as_str()),
                ("created_by_version", env!("CARGO_PKG_VERSION")),
            ],
        )
        .context("failed to create history artifact metadata")?;
    }
    transaction
        .commit()
        .context("failed to commit history artifact metadata transaction")?;
    Ok(())
}

pub(crate) fn write_export_metadata(connection: &mut Connection) -> rusqlite::Result<()> {
    write_metadata(
        connection,
        &[
            ("artifact", "history-export"),
            ("format_version", FORMAT_VERSION),
            ("created_by_version", env!("CARGO_PKG_VERSION")),
        ],
    )
}

fn write_metadata(connection: &mut Connection, values: &[(&str, &str)]) -> rusqlite::Result<()> {
    let transaction = connection.transaction()?;
    write_metadata_in_transaction(&transaction, values)?;
    transaction.commit()
}

fn write_metadata_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    values: &[(&str, &str)],
) -> rusqlite::Result<()> {
    transaction.execute(
        r#"CREATE TABLE arf_metadata (
            key TEXT PRIMARY KEY NOT NULL,
            value TEXT NOT NULL
        )"#,
        [],
    )?;
    {
        let mut statement =
            transaction.prepare("INSERT INTO arf_metadata (key, value) VALUES (?1, ?2)")?;
        for (key, value) in values {
            statement.execute([key, value])?;
        }
    }
    Ok(())
}

fn metadata_table_exists(connection: &Connection) -> Result<bool> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [METADATA_TABLE],
            |row| row.get(0),
        )
        .context("failed to check for arf artifact metadata")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sidecar_path(path: &Path, suffix: &str) -> std::path::PathBuf {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        sidecar.into()
    }

    fn connection_with_metadata(entries: &[(&str, &str)]) -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                r#"CREATE TABLE arf_metadata (
                    key TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL
                )"#,
            )
            .unwrap();
        for (key, value) in entries {
            connection
                .execute(
                    "INSERT INTO arf_metadata (key, value) VALUES (?1, ?2)",
                    [key, value],
                )
                .unwrap();
        }
        connection
    }

    #[test]
    fn absent_metadata_table_is_legacy() {
        let connection = Connection::open_in_memory().unwrap();
        assert_eq!(read_artifact(&connection).unwrap(), HistoryArtifact::Legacy);
    }

    #[test]
    fn unknown_metadata_keys_are_ignored() {
        let connection = connection_with_metadata(&[
            ("artifact", "history"),
            ("format_version", "1"),
            ("created_by_version", "test"),
            ("history_kind", "r"),
            ("future_key", "future value"),
        ]);
        assert_eq!(
            read_artifact(&connection).unwrap(),
            HistoryArtifact::History(HistoryKind::R)
        );
    }

    #[test]
    fn invalid_or_missing_required_metadata_is_rejected() {
        for entries in [
            vec![("artifact", "history")],
            vec![
                ("artifact", "history"),
                ("format_version", "1"),
                ("created_by_version", "test"),
            ],
            vec![
                ("artifact", "history"),
                ("format_version", "one"),
                ("created_by_version", "test"),
                ("history_kind", "r"),
            ],
            vec![
                ("artifact", "history"),
                ("format_version", "1"),
                ("created_by_version", "test"),
                ("history_kind", "browse"),
            ],
            vec![
                ("artifact", "history-export"),
                ("format_version", "1"),
                ("created_by_version", ""),
            ],
        ] {
            let connection = connection_with_metadata(&entries);
            assert!(read_artifact(&connection).is_err(), "{entries:?}");
        }
    }

    #[test]
    fn future_format_versions_are_rejected() {
        let connection = connection_with_metadata(&[
            ("artifact", "history"),
            ("format_version", "2"),
            ("created_by_version", "test"),
            ("history_kind", "r"),
        ]);
        let error = read_artifact(&connection).unwrap_err().to_string();
        assert!(error.contains("unsupported arf artifact format version 2"));
    }

    #[test]
    fn metadata_initialization_is_idempotent_and_rejects_a_different_kind() {
        let mut connection = Connection::open_in_memory().unwrap();
        initialize_history_metadata(&mut connection, HistoryKind::R).unwrap();
        initialize_history_metadata(&mut connection, HistoryKind::R).unwrap();
        assert_eq!(
            read_artifact(&connection).unwrap(),
            HistoryArtifact::History(HistoryKind::R)
        );

        let error = initialize_history_metadata(&mut connection, HistoryKind::Shell).unwrap_err();
        assert!(error.to_string().contains("history database kind mismatch"));
    }

    #[test]
    fn metadata_initialization_rejects_partial_existing_table_without_backfill() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                r#"CREATE TABLE arf_metadata (
                    key TEXT PRIMARY KEY NOT NULL,
                    value TEXT NOT NULL
                );
                INSERT INTO arf_metadata (key, value) VALUES ('artifact', 'history');"#,
            )
            .unwrap();

        let error = initialize_history_metadata(&mut connection, HistoryKind::R).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing required key 'format_version'")
        );
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM arf_metadata", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn staged_history_is_published_with_complete_metadata_and_no_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("history.db");
        let staged = stage_history_database(dir.path(), HistoryKind::R).unwrap();
        let staged_path = staged.path().to_path_buf();

        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(
                !sidecar_path(&staged_path, suffix).exists(),
                "staged sidecar still exists: {}",
                sidecar_path(&staged_path, suffix).display()
            );
        }
        publish_staged_history_database(staged, &target, HistoryKind::R).unwrap();

        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(!sidecar_path(&target, suffix).exists());
        }
        assert_eq!(
            read_artifact_from_path(&target).unwrap(),
            HistoryArtifact::History(HistoryKind::R)
        );
    }

    #[test]
    fn staged_publication_validates_a_same_kind_winner_and_rejects_other_kind() {
        let dir = tempfile::tempdir().unwrap();

        let same_kind_target = dir.path().join("same.db");
        let winner = stage_history_database(dir.path(), HistoryKind::R).unwrap();
        publish_staged_history_database(winner, &same_kind_target, HistoryKind::R).unwrap();
        let contender = stage_history_database(dir.path(), HistoryKind::R).unwrap();
        publish_staged_history_database(contender, &same_kind_target, HistoryKind::R).unwrap();

        let other_kind_target = dir.path().join("other.db");
        let winner = stage_history_database(dir.path(), HistoryKind::Shell).unwrap();
        publish_staged_history_database(winner, &other_kind_target, HistoryKind::Shell).unwrap();
        let contender = stage_history_database(dir.path(), HistoryKind::R).unwrap();
        let error = publish_staged_history_database(contender, &other_kind_target, HistoryKind::R)
            .unwrap_err();
        assert!(error.to_string().contains("history database kind mismatch"));
        assert_eq!(
            read_artifact_from_path(&other_kind_target).unwrap(),
            HistoryArtifact::History(HistoryKind::Shell)
        );
    }

    #[test]
    fn preparation_creates_nested_parent_directories() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested/path/history.db");

        prepare_history_path(&target, HistoryKind::R).unwrap();

        assert_eq!(
            read_artifact_from_path(&target).unwrap(),
            HistoryArtifact::History(HistoryKind::R)
        );
    }
}
