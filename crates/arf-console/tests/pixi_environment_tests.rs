//! Real Pixi regressions run only by the dedicated integration workflow.
#![allow(dead_code)]

#[path = "tui_tests/support.rs"]
mod support;

use anyhow::{Context, Result, ensure};
use support::{PROMPT, Terminal, run_case_with};

#[test]
#[ignore = "requires the dedicated activated Pixi environment"]
fn pixi_compiled_packages_survive_restart_in_a_different_workspace() -> Result<()> {
    run_restart_case("pixi-compiled-restart-configured", false)?;
    run_restart_case("pixi-compiled-restart-explicit", true)
}

fn run_restart_case(name: &str, explicit: bool) -> Result<()> {
    let home = std::env::var("ARF_TEST_PIXI_R_HOME").context("verified Pixi R_HOME")?;
    let other = tempfile::tempdir()?;
    std::fs::write(
        other.path().join("pixi.toml"),
        r"[workspace]
name='other'",
    )?;
    let expression = r#"stopifnot(normalizePath(R.home()) == normalizePath(Sys.getenv('ARF_TEST_PIXI_R_HOME'))); stopifnot(abs(stats::sd(1:3) - 1) < 1e-10); stopifnot(jsonlite::fromJSON('{"answer":42}')$answer == 42); stopifnot(nchar(curl::curl_version()$version) > 0); cat(paste0('PIXI_', 'COMPILED_OK'), '\n')"#;
    let config = if explicit {
        support::DEFAULT_CONFIG.to_owned()
    } else {
        format!(
            r"{}
[experimental]
r_source_overrides=[{{provider='pixi'}}]
",
            support::DEFAULT_CONFIG
        )
    };
    let builder = Terminal::builder(name)
        .config(config)
        .env("ARF_TEST_PIXI_R_HOME", home)
        .args([
            "--no-r-auto-discovery",
            "--no-auto-match",
            "--no-completion",
        ]);
    let builder = if explicit {
        builder.args(["--r-provider", "pixi"])
    } else {
        builder.with_r_source_overrides()
    };
    run_case_with(builder, |terminal| {
        terminal.wait_for_first_prompt()?;
        terminal.submit(expression, "PIXI_COMPILED_OK", PROMPT)?;
        terminal.submit(
            &format!(
                r"setwd({}); cat(paste0('CWD_', 'CHANGED'), '\n')",
                serde_json::to_string(&other.path().display().to_string())?
            ),
            "CWD_CHANGED",
            PROMPT,
        )?;
        let checkpoint = terminal.checkpoint()?;
        terminal.enter(":restart!")?;
        terminal.wait_for("restarted Pixi prompt", |state, line| {
            state.exited.is_none()
                && terminal.output_since(checkpoint).is_ok_and(|output| {
                    output.contains("Restarting R session...") && output.contains("is ready.")
                })
                && line.trim_end() == PROMPT
        })?;
        let checkpoint = terminal.checkpoint()?;
        terminal.enter(expression)?;
        terminal.wait_for("fresh compiled output after restart", |_, line| {
            line.trim_end() == PROMPT
                && terminal
                    .output_since(checkpoint)
                    .is_ok_and(|output| output.contains("PIXI_COMPILED_OK"))
        })?;
        ensure!(
            terminal.state()?.exited.is_none(),
            "replacement process exited"
        );
        terminal.quit()
    })
}
