use super::support::{
    ERROR_PROMPT, PROMPT, Terminal, build_formatter_fixture_path, run_case, run_case_with,
};
use crate::common::{HistoryRow, wait_for_history_rows};
use anyhow::{Result, ensure};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn wait_for_history(
    history_dir: &Path,
    description: &str,
    predicate: impl Fn(&[HistoryRow]) -> bool,
) -> Result<Vec<HistoryRow>> {
    wait_for_history_rows(
        &history_dir.join("r.db"),
        description,
        Duration::from_secs(30),
        Duration::from_millis(25),
        predicate,
    )
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
            terminal.wait_for_prompt(Some("# [arf] Reprex: on"), "[reprex] ARF>")?;
            terminal.enter(":reprex format")?;
            // Paste only once the next prompt is drawn. On Windows, input that
            // arrives before the line editor enables bracketed paste loses
            // its paste markers, so each pasted line would be submitted alone.
            terminal.wait_for_prompt(Some("# [arf] Reprex: format"), "[format] ARF>")?;

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
fn formatter_failure_updates_lifecycle_without_evaluation() -> Result<()> {
    for formatter in ["air", "arity"] {
        formatter_failure_scenario(formatter)?;
    }
    Ok(())
}

fn formatter_failure_scenario(formatter: &str) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let history = temp.path().join("history");
    let path = build_formatter_fixture_path(&temp.path().join("bin"), formatter)?;
    let config = r#"
[startup]
reprex = "format"

[reprex]
formatter = "__FORMATTER__"

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
"#
    .replace("__FORMATTER__", formatter);
    run_case_with(
        Terminal::builder(format!("formatter-failure-{formatter}"))
            .args(["--no-auto-match"])
            .config(config)
            .env("PATH", path)
            .env("FORMATTER_FIXTURE_MODE", "failure")
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
                        .any(|row| row.command == "42" && row.exit_status == Some(1))
                })?;
            ensure!(
                failed_rows
                    .iter()
                    .any(|row| { row.command == "42" && row.exit_status == Some(1) })
            );

            let follow_up = "cat('FOLLOW_UP_ONE\\n'); 1";
            let before_follow_up = terminal.state()?;
            terminal.enter(follow_up)?;
            terminal.wait_for("follow-up success", |state, line| {
                state.text.contains("FOLLOW_UP_ONE")
                    && state.text.contains("[1] 1")
                    && line.trim_end().ends_with("ARF>")
            })?;
            wait_for_history(&history, "history forget after follow-up", |rows| {
                rows.iter()
                    .any(|row| row.command.contains("FOLLOW_UP_ONE") && row.exit_status == Some(0))
                    && !rows.iter().any(|row| row.command == "42")
            })?;
            let follow_up_state = terminal.state()?;
            ensure!(
                follow_up_state.cursor.y > before_follow_up.cursor.y,
                "follow-up evaluation did not advance to a fresh screen row: {follow_up_state:?}"
            );
            let lines = follow_up_state.text.lines().collect::<Vec<_>>();
            let follow_up_marker = lines.iter().position(|line| line.contains("FOLLOW_UP_ONE"));
            let result = lines.iter().rposition(|line| line.contains("[1] 1"));
            let prompt = lines
                .iter()
                .rposition(|line| line.trim_end().ends_with("ARF>"));
            ensure!(
                follow_up_marker
                    .zip(result)
                    .zip(prompt)
                    .is_some_and(|((marker, result), prompt)| marker < result && result < prompt),
                "follow-up marker and result were not visible in order before the fresh prompt: {follow_up_state:?}"
            );
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
                rows.iter()
                    .any(|row| row.command.contains("dplyr::select") && row.exit_status == Some(1))
            })?;
            terminal.submit("1", "[1] 1", PROMPT)?;
            let rows = wait_for_history(history.path(), "successful rlang follow-up", |rows| {
                rows.iter()
                    .any(|row| row.command.contains("dplyr::select") && row.exit_status == Some(1))
                    && rows
                        .iter()
                        .any(|row| row.command == "42" && row.exit_status == Some(0))
            })?;
            ensure!(rows.iter().any(|row| {
                row.command.contains("dplyr::select") && row.exit_status == Some(1)
            }));
            terminal.quit()
        },
    )
}
