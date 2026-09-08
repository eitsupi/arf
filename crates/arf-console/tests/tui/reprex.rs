use super::support::{ERROR_PROMPT, PROMPT, Terminal, run_case, run_case_with};
use anyhow::{Result, bail, ensure};
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

type HistoryRow = (String, Option<i64>);

fn history_rows(history_dir: &Path) -> Result<Vec<HistoryRow>> {
    let path = history_dir.join("r.db");
    ensure!(
        path.is_file(),
        "history database is missing: {}",
        path.display()
    );
    let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement =
        connection.prepare("SELECT command_line, exit_status FROM history ORDER BY id")?;
    Ok(statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn wait_for_history(
    history_dir: &Path,
    description: &str,
    predicate: impl Fn(&[HistoryRow]) -> bool,
) -> Result<Vec<HistoryRow>> {
    let started = Instant::now();
    let mut last_error = None;
    loop {
        match history_rows(history_dir) {
            Ok(rows) if predicate(&rows) => return Ok(rows),
            Ok(_) => {}
            Err(error) => last_error = Some(error),
        }
        if started.elapsed() >= Duration::from_secs(30) {
            return match last_error {
                Some(error) => Err(error).map_err(|error| {
                    anyhow::anyhow!("timed out waiting for {description}: {error}")
                }),
                None => bail!("timed out waiting for {description}"),
            };
        }
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn reprex_paste_strips_output_lines() -> Result<()> {
    ensure!(
        Command::new("air").arg("--version").status()?.success(),
        "Air CLI is required for reprex paste migration test"
    );
    run_case(
        "reprex-paste",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            terminal.enter(":reprex on")?;
            terminal.wait_for("reprex mode enabled", |state, _| {
                state.text.contains("Reprex: on")
            })?;
            terminal.enter(":reprex format")?;
            terminal.wait_for("reprex format enabled", |state, _| {
                state.text.contains("Reprex: format")
            })?;

            terminal.write(
                "\x1b[200~x <- 42\n#> STALE_OUTPUT_42\nx + 1\n#> STALE_OUTPUT_43\x1b[201~\r",
            )?;
            terminal.wait_for("reprex paste results", |state, line| {
                state.text.contains("#> [1] 43")
                    && !state.text.contains("STALE_OUTPUT_42")
                    && !state.text.contains("STALE_OUTPUT_43")
                    && line.trim_end().ends_with("ARF>")
            })?;
            let paste_state = terminal.state()?;
            ensure!(
                !paste_state.text.contains("STALE_OUTPUT_42")
                    && !paste_state.text.contains("STALE_OUTPUT_43"),
                "reprex paste left stale output markers on screen: {}",
                paste_state.text
            );
            terminal.enter("cat('REPREX_X_42\\n'); x")?;
            terminal.wait_for("reprex assignment value", |state, line| {
                state.text.contains("REPREX_X_42")
                    && state.text.contains("#> [1] 42")
                    && line.trim_end().ends_with("ARF>")
            })
        },
    )?;
    Ok(())
}

#[test]
#[cfg(unix)]
fn formatter_failure_updates_lifecycle_without_evaluation() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir()?;
    let bin = temp.path().join("bin");
    std::fs::create_dir(&bin)?;
    let arity = bin.join("arity");
    std::fs::write(
        &arity,
        r##"#!/bin/sh
case "$1" in
  --version) echo 'arity test stub'; exit 0 ;;
  format)
    input=$(cat)
    if [ "$input" = 42 ]; then
      echo 'synthetic formatter failure' >&2
      exit 17
    fi
    printf '%s' "$input"
    exit 0
    ;;
  *) exit 18 ;;
esac
"##,
    )?;
    let mut permissions = std::fs::metadata(&arity)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&arity, permissions)?;

    let current_path = std::env::var_os("PATH").unwrap_or_default();
    let path =
        std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&current_path)))?
            .to_string_lossy()
            .into_owned();
    let history = temp.path().join("history");
    let config = r#"
[startup]
reprex = "format"

[reprex]
formatter = "arity"

[prompt]
format = "{status}{duration}ARF> "

[prompt.status.symbol]
error = "ERR "

[experimental.prompt_duration]
threshold_ms = 0

[experimental.prompt_spinner]
frames = ""

[experimental.history_forget]
enabled = true
delay = 1
on_exit_only = false
"#;
    run_case_with(
        Terminal::builder("formatter-failure")
            .args(["--no-auto-match"])
            .config(config)
            .env("PATH", path)
            .history_dir(&history),
        |terminal| {
            terminal.wait_for("formatter prompt", |_, line| {
                line.trim_end().ends_with("ARF>")
            })?;
            terminal.enter("1")?;
            terminal.wait_for("formatted success with duration", |state, line| {
                state.text.contains("[1] 1")
                    && line.contains("ms")
                    && line.trim_end().ends_with("ARF>")
            })?;

            let failed_checkpoint = terminal.checkpoint()?;
            terminal.enter("42")?;
            terminal.wait_for("formatter failure prompt", |state, line| {
                state.text.contains("synthetic formatter failure")
                    && line.contains("ERR ")
                    && !line.contains("ms")
                    && line.trim_end().ends_with("ARF>")
            })?;
            let failure_output = terminal.output_since(failed_checkpoint)?;
            ensure!(
                failure_output.contains("synthetic formatter failure"),
                "formatter failure was not emitted after the command"
            );
            ensure!(
                !failure_output.contains("[1] 42"),
                "formatter-rejected code must not be evaluated"
            );
            let failed_rows =
                wait_for_history(&history, "failed formatter history entry", |rows| {
                    rows.iter()
                        .any(|(command, status)| command == "42" && *status == Some(1))
                })?;
            ensure!(
                failed_rows
                    .iter()
                    .any(|(command, status)| { command == "42" && *status == Some(1) })
            );

            let follow_up_checkpoint = terminal.checkpoint()?;
            let follow_up = "cat('FOLLOW_UP_ONE\\n'); 1";
            terminal.enter(follow_up)?;
            terminal.wait_for("follow-up success", |state, line| {
                state.text.contains("FOLLOW_UP_ONE")
                    && state.text.contains("[1] 1")
                    && line.trim_end().ends_with("ARF>")
            })?;
            ensure!(
                terminal
                    .output_since(follow_up_checkpoint)?
                    .contains("[1] 1"),
                "follow-up result was not emitted after its checkpoint"
            );
            wait_for_history(&history, "history forget after follow-up", |rows| {
                rows.iter().any(|(command, status)| {
                    command.contains("FOLLOW_UP_ONE") && *status == Some(0)
                }) && !rows.iter().any(|(command, _)| command == "42")
            })?;
            terminal.quit()
        },
    )
}

#[test]
fn rlang_error_detection_records_failure_status() -> Result<()> {
    let dplyr_check = Command::new("Rscript")
        .args([
            "-e",
            "if (!requireNamespace('dplyr', quietly=TRUE)) quit(status=1)",
        ])
        .status()?;
    ensure!(
        dplyr_check.success(),
        "dplyr is required for rlang error migration test"
    );
    let history = tempfile::tempdir()?;
    run_case_with(
        Terminal::builder("rlang-error")
            .args(["--no-auto-match"])
            .history_dir(history.path()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.submit("42", "[1] 42", PROMPT)?;
            terminal.enter("mtcars |> dplyr::select(nonexistent_column)")?;
            terminal.wait_for("rlang missing-column error", |state, line| {
                state.text.contains("doesn't exist") && line.trim_end() == ERROR_PROMPT
            })?;
            wait_for_history(history.path(), "rlang error history entry", |rows| {
                rows.iter().any(|(command, status)| {
                    command.contains("dplyr::select") && *status == Some(1)
                })
            })?;
            terminal.submit("1", "[1] 1", PROMPT)?;
            let rows = wait_for_history(history.path(), "successful rlang follow-up", |rows| {
                rows.iter().any(|(command, status)| {
                    command.contains("dplyr::select") && *status == Some(1)
                }) && rows
                    .iter()
                    .any(|(command, status)| command == "42" && *status == Some(0))
            })?;
            ensure!(rows.iter().any(|(command, status)| {
                command.contains("dplyr::select") && *status == Some(1)
            }));
            terminal.quit()
        },
    )
}
