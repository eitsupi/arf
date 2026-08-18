use super::super::*;
use chrono::{DateTime, Utc};
use reedline::{HistoryItem, SearchDirection, SearchQuery, SqliteBackedHistory};
use std::path::Path;
use std::time::Duration;
use tempfile::{NamedTempFile, TempDir};

pub(super) fn entry(command: &str) -> ImportEntry {
    ImportEntry::new(command)
}

pub(super) fn r(command: &str) -> ImportEntry {
    entry(command).with_mode(ImportMode::R)
}

pub(super) fn shell(command: &str) -> ImportEntry {
    entry(command).with_mode(ImportMode::Shell)
}

pub(super) fn browse(command: &str) -> ImportEntry {
    entry(command).with_mode(ImportMode::Browse)
}

pub(super) trait EntryExt {
    fn at(self, timestamp: DateTime<Utc>) -> Self;
    fn with_metadata(self, json: &str) -> Self;
    fn with_hostname(self, hostname: &str) -> Self;
    fn with_standard_fields(self) -> Self;
}

impl EntryExt for ImportEntry {
    fn at(mut self, timestamp: DateTime<Utc>) -> Self {
        self.item.start_timestamp = Some(timestamp);
        self
    }

    fn with_metadata(mut self, json: &str) -> Self {
        self.item.more_info = Some(serde_json::from_str(json).unwrap());
        self
    }

    fn with_hostname(mut self, hostname: &str) -> Self {
        self.item.hostname = Some(hostname.to_owned());
        self
    }

    fn with_standard_fields(mut self) -> Self {
        self.item.session_id = reedline::Reedline::create_history_session_id();
        self.item.hostname = Some("fixture-host".to_owned());
        self.item.cwd = Some("/fixture/cwd".to_owned());
        self.item.duration = Some(Duration::from_millis(1234));
        self.item.exit_status = Some(17);
        self
    }
}

pub(super) fn timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

pub(super) fn text_fixture(contents: &str) -> NamedTempFile {
    use std::io::Write;
    let mut file = NamedTempFile::new().unwrap();
    file.write_all(contents.as_bytes()).unwrap();
    file
}

#[derive(Default)]
pub(super) struct ImportOptions<'a> {
    pub hostname_override: Option<&'a str>,
    pub skip_duplicates: bool,
}

pub(super) struct ImportFixture {
    pub targets: ImportTargets,
    _dir: TempDir,
}

impl ImportFixture {
    pub fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let targets = ImportTargets {
            r_history: HistoryStore::open(dir.path().join("r.db"), None, None).unwrap(),
            shell_history: HistoryStore::open(dir.path().join("shell.db"), None, None).unwrap(),
        };
        Self { targets, _dir: dir }
    }

    pub fn import(&mut self, entries: impl IntoIterator<Item = ImportEntry>) -> ImportResult {
        self.import_with(entries, ImportOptions::default())
    }

    pub fn import_with(
        &mut self,
        entries: impl IntoIterator<Item = ImportEntry>,
        options: ImportOptions<'_>,
    ) -> ImportResult {
        import_entries(
            &mut self.targets,
            entries.into_iter().collect(),
            options.hostname_override,
            options.skip_duplicates,
        )
        .unwrap()
    }

    pub fn r_items(&self) -> Vec<HistoryItem<HistoryExtraInfo>> {
        items(&self.targets.r_history)
    }

    pub fn shell_items(&self) -> Vec<HistoryItem<HistoryExtraInfo>> {
        items(&self.targets.shell_history)
    }
}

fn items(store: &HistoryStore) -> Vec<HistoryItem<HistoryExtraInfo>> {
    store
        .search(SearchQuery::everything(SearchDirection::Forward, None))
        .unwrap()
        .into_iter()
        .map(|item| store.load_with_metadata(item.id.unwrap()).unwrap())
        .collect()
}

#[derive(Default)]
pub(super) struct DbRow<'a> {
    pub command: &'a str,
    pub timestamp_ms: Option<i64>,
    pub session_id: Option<i64>,
    pub hostname: Option<&'a str>,
    pub cwd: Option<&'a str>,
    pub duration_ms: Option<i64>,
    pub exit_status: Option<i64>,
    pub metadata: Option<&'a str>,
}

#[derive(Clone, Copy)]
pub(super) enum ExportColumns {
    Full,
    LegacyMinimal,
}

pub(super) fn create_reedline_db(path: &Path, rows: &[HistoryItem<HistoryExtraInfo>]) {
    let mut db = SqliteBackedHistory::with_file(path.to_owned(), None, None).unwrap();
    for row in rows {
        db.save_with_extra(row.clone()).unwrap();
    }
}

pub(super) fn create_export_table(
    db: &rusqlite::Connection,
    table: &str,
    columns: ExportColumns,
    rows: &[DbRow<'_>],
) {
    match columns {
        ExportColumns::Full => db
            .execute(
                &format!(
                    r#"CREATE TABLE "{table}" (
                        id INTEGER PRIMARY KEY,
                        command_line TEXT NOT NULL,
                        start_timestamp INTEGER,
                        session_id INTEGER,
                        hostname TEXT,
                        cwd TEXT,
                        duration_ms INTEGER,
                        exit_status INTEGER,
                        more_info TEXT
                    )"#
                ),
                [],
            )
            .unwrap(),
        ExportColumns::LegacyMinimal => db
            .execute(
                &format!(
                    r#"CREATE TABLE "{table}" (
                        id INTEGER PRIMARY KEY,
                        command_line TEXT NOT NULL,
                        start_timestamp INTEGER
                    )"#
                ),
                [],
            )
            .unwrap(),
    };

    for (index, row) in rows.iter().enumerate() {
        let id = (index + 1) as i64;
        match columns {
            ExportColumns::Full => db
                .execute(
                    &format!(
                        r#"INSERT INTO "{table}"
                           (id, command_line, start_timestamp, session_id, hostname, cwd,
                            duration_ms, exit_status, more_info)
                           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#
                    ),
                    rusqlite::params![
                        id,
                        row.command,
                        row.timestamp_ms,
                        row.session_id,
                        row.hostname,
                        row.cwd,
                        row.duration_ms,
                        row.exit_status,
                        row.metadata,
                    ],
                )
                .unwrap(),
            ExportColumns::LegacyMinimal => db
                .execute(
                    &format!(
                        r#"INSERT INTO "{table}" (id, command_line, start_timestamp)
                           VALUES (?, ?, ?)"#
                    ),
                    rusqlite::params![id, row.command, row.timestamp_ms],
                )
                .unwrap(),
        };
    }
}

pub(super) fn idless(mut item: HistoryItem<HistoryExtraInfo>) -> HistoryItem<HistoryExtraInfo> {
    item.id = None;
    item
}
