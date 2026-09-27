//! Versioned metadata for arf-owned history database artifacts.

use anyhow::{Context, Result, bail};
use rusqlite::Connection;
use std::collections::HashMap;
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
    let table_exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            [METADATA_TABLE],
            |row| row.get(0),
        )
        .context("failed to check for arf artifact metadata")?;
    if !table_exists {
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

pub(crate) fn write_history_metadata(
    connection: &mut Connection,
    kind: HistoryKind,
) -> rusqlite::Result<()> {
    write_metadata(
        connection,
        &[
            ("artifact", "history"),
            ("format_version", FORMAT_VERSION),
            ("history_kind", kind.as_str()),
            ("created_by_version", env!("CARGO_PKG_VERSION")),
        ],
    )
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
    transaction.commit()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
