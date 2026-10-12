//! Helpers shared by the integration test binaries.
//!
//! Every `tests/*` target is compiled separately, so each binary that needs
//! these helpers includes this module itself and uses only part of it.
#![allow(dead_code)]

use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Call `check` every `interval` until it returns `Some`, or give up once
/// `timeout` has elapsed. `check` runs at least once and is evaluated before
/// the deadline, so a condition that becomes true on the last tick is seen.
pub fn poll_until<T>(
    timeout: Duration,
    interval: Duration,
    mut check: impl FnMut() -> Option<T>,
) -> Option<T> {
    let started = Instant::now();
    loop {
        if let Some(value) = check() {
            return Some(value);
        }
        if started.elapsed() >= timeout {
            return None;
        }
        thread::sleep(interval);
    }
}

/// Like [`poll_until`], for checks which can fail. An error stops polling
/// immediately; `Ok(None)` means the deadline passed without a result.
pub fn try_poll_until<T>(
    timeout: Duration,
    interval: Duration,
    mut check: impl FnMut() -> Result<Option<T>>,
) -> Result<Option<T>> {
    let started = Instant::now();
    loop {
        if let Some(value) = check()? {
            return Ok(Some(value));
        }
        if started.elapsed() >= timeout {
            return Ok(None);
        }
        thread::sleep(interval);
    }
}

/// Wait for `child` to exit. When it is still running after `timeout`, kill
/// and reap it and return `false`; the caller decides how to report that.
pub fn wait_for_child_exit(
    child: &mut Child,
    timeout: Duration,
    interval: Duration,
) -> std::io::Result<bool> {
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(true);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(false);
        }
        thread::sleep(interval);
    }
}

/// Guard for [`spawn_watchdog`]; dropping it disarms the watchdog.
pub struct Watchdog {
    _disarm: mpsc::Sender<()>,
}

/// Terminate the whole test process if the returned guard is still alive after
/// `timeout`. `describe` runs on the watchdog thread and returns the message
/// to print before exiting; it must not touch locks held by the test.
pub fn spawn_watchdog(
    timeout: Duration,
    describe: impl FnOnce() -> String + Send + 'static,
) -> Watchdog {
    let (disarm, deadline) = mpsc::channel::<()>();
    thread::spawn(move || {
        if matches!(
            deadline.recv_timeout(timeout),
            Err(mpsc::RecvTimeoutError::Timeout)
        ) {
            let message = describe();
            // Bypass libtest's per-test capture: process::exit does not flush it.
            let _ = std::io::Write::write_all(&mut std::io::stderr().lock(), message.as_bytes());
            std::process::exit(1);
        }
    });
    Watchdog { _disarm: disarm }
}

/// Compile a native test fixture from `tests/fixtures/<name>.rs` and return
/// the executable's bytes.
pub fn compile_fixture(name: &str) -> std::result::Result<Vec<u8>, String> {
    let build_dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let fixture_source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(format!("{name}.rs"));
    let fixture = build_dir
        .path()
        .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(compiler)
        .arg(fixture_source)
        .arg("--edition=2024")
        .arg("-D")
        .arg("warnings")
        .arg("-o")
        .arg(&fixture)
        .output()
        .map_err(|error| format!("failed to run rustc for the {name} test fixture: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "failed to compile the {name} test fixture: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    fs::read(fixture).map_err(|error| format!("failed to read the {name} test fixture: {error}"))
}

/// Write an executable file, creating its parent directory when needed.
pub fn write_executable(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// An `arf` command which ignores R-selection, history and config variables
/// inherited from the developer's or CI's environment.
pub fn sanitized_arf_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_arf"));
    for variable in [
        "ARF_R_HOME",
        "ARF_R_VERSION",
        "ARF_R_PROVIDER",
        "ARF_HISTORY_DIR",
        "ARF_CONFIG",
    ] {
        command.env_remove(variable);
    }
    command
}

/// One saved command from arf's history database.
#[derive(Debug, Clone)]
pub struct HistoryRow {
    pub command: String,
    pub exit_status: Option<i64>,
    pub hostname: Option<String>,
    pub cwd: Option<String>,
}

/// Read every history row, oldest first. Fails while the database file is
/// missing or not yet readable.
pub fn read_history_rows(db_path: &Path) -> Result<Vec<HistoryRow>> {
    anyhow::ensure!(
        db_path.is_file(),
        "history database is missing: {}",
        db_path.display()
    );
    let connection = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = connection
        .prepare("SELECT command_line, exit_status, hostname, cwd FROM history ORDER BY id")?;
    let rows = statement
        .query_map([], |row| {
            Ok(HistoryRow {
                command: row.get(0)?,
                exit_status: row.get(1)?,
                hostname: row.get(2)?,
                cwd: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Poll the history database until `predicate` accepts its rows. Read errors
/// are retried until the deadline and reported if they persist.
pub fn wait_for_history_rows(
    db_path: &Path,
    description: &str,
    timeout: Duration,
    interval: Duration,
    predicate: impl Fn(&[HistoryRow]) -> bool,
) -> Result<Vec<HistoryRow>> {
    let mut last_error = None;
    let rows = poll_until(timeout, interval, || match read_history_rows(db_path) {
        Ok(rows) if predicate(&rows) => Some(rows),
        Ok(_) => None,
        Err(error) => {
            last_error = Some(error);
            None
        }
    });
    match (rows, last_error) {
        (Some(rows), _) => Ok(rows),
        (None, Some(error)) => Err(anyhow::anyhow!(
            "timed out waiting for {description}: {error}"
        )),
        (None, None) => anyhow::bail!("timed out waiting for {description}"),
    }
}
