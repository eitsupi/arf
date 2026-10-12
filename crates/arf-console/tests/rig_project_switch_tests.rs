//! Real rig project R-version switching regressions run in the dedicated macOS workflow.
#![allow(dead_code)]

#[path = "tui_tests/support.rs"]
mod support;

use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use support::{PROMPT, Terminal, run_case_with};

fn state() -> Result<Value> {
    let path = std::env::var_os("ARF_RIG_SWITCH_STATE")
        .context("switch state path from the integration harness")?;
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn required_path(value: &Value, key: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(value[key].as_str().with_context(|| {
        format!("missing path in switch state: {key}")
    })?))
}

fn wait_for_replacement(
    terminal: &Terminal,
    checkpoint: usize,
    description: &str,
) -> Result<String> {
    terminal.wait_for(description, |state, line| {
        state.exited.is_none()
            && line.trim_end() == PROMPT
            && terminal.output_since(checkpoint).is_ok_and(|output| {
                (output.contains("Restarting R session...")
                    || output.contains("Restarting with R "))
                    && output.contains("is ready.")
            })
    })?;
    terminal.output_since(checkpoint)
}

fn capture_proof(terminal: &Terminal, script: &Path, directory: &Path, stage: &str) -> Result<()> {
    let output = directory.join(format!("{stage}.txt"));
    if output.exists() {
        fs::remove_file(&output)?;
    }
    let script = serde_json::to_string(&script.to_string_lossy())?;
    let output_literal = serde_json::to_string(&output.to_string_lossy())?;
    let expression =
        format!(r#"capture.output(source({script}, local = TRUE), file = {output_literal})"#);
    let response = terminal
        .start_ipc(&["eval", &expression, "--timeout", "30000"])?
        .finish()?;
    ensure!(
        response["error"].is_null(),
        "proof evaluation failed: {response}"
    );
    ensure!(output.is_file(), "R did not write the {stage} proof");
    Ok(())
}

#[test]
#[ignore = "requires rig 0.11.0 and three isolated R installations"]
fn rig_project_restart_and_switch_preserve_the_selected_r_context() -> Result<()> {
    let state = state()?;
    let project = required_path(&state, "project")?;
    let user_home = required_path(&state, "user_home")?;
    let script = required_path(&state, "proof_script")?;
    let proof_dir = required_path(&state, "proof_dir")?;
    let startup_dyld = state["base_dyld_library_path"]
        .as_str()
        .context("initial DYLD_LIBRARY_PATH in switch state")?;
    let selected_home = PathBuf::from(
        state["r_homes"]["4.6.1"]
            .as_str()
            .context("base R_HOME in switch state")?,
    );
    let startup_paths =
        std::env::split_paths(std::ffi::OsStr::new(startup_dyld)).collect::<Vec<_>>();
    ensure!(
        startup_paths.first() == Some(&selected_home.join("lib")),
        "the initial DYLD path must start with the base R lib directory: {startup_paths:?}"
    );
    ensure!(
        startup_paths.len() >= 2 && startup_paths[1].is_dir(),
        "the initial DYLD path must retain an unrelated directory: {startup_paths:?}"
    );
    ensure!(
        user_home.join(".Renviron").is_file(),
        "isolated HOME has no .Renviron"
    );
    fs::create_dir_all(&proof_dir)?;

    run_case_with(
        Terminal::builder("rig-project-r-version-switch")
            .cwd(&project)
            .vanilla(false)
            .artifacts(proof_dir.join("tui"))
            .env("HOME", user_home.to_string_lossy())
            .env("DYLD_LIBRARY_PATH", startup_dyld)
            .args([
                "--r-provider",
                "rig-project",
                "--no-r-auto-discovery",
                "--no-auto-match",
                "--no-completion",
                "--with-ipc",
                "--ipc-eval-unrestricted",
            ]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            capture_proof(terminal, &script, &proof_dir, "initial")?;

            let checkpoint = terminal.checkpoint()?;
            terminal.enter(":restart!")?;
            wait_for_replacement(terminal, checkpoint, "same-root restart")?;
            capture_proof(terminal, &script, &proof_dir, "restart")?;

            let checkpoint = terminal.checkpoint()?;
            terminal.enter(":switch! 4.6.0")?;
            wait_for_replacement(terminal, checkpoint, "same-minor R switch")?;
            capture_proof(terminal, &script, &proof_dir, "switch-same-minor")?;

            let checkpoint = terminal.checkpoint()?;
            terminal.enter(":switch! 99.99.99")?;
            terminal.wait_for(
                "rejected R version leaves the session running",
                |state, line| {
                    state.exited.is_none()
                        && line.trim_end() == PROMPT
                        && terminal
                            .output_since(checkpoint)
                            .is_ok_and(|output| output.contains("Error:"))
                },
            )?;
            capture_proof(terminal, &script, &proof_dir, "same-minor-after-invalid")?;

            terminal.submit(
                r#"stopifnot(nzchar(Sys.getenv("RVENV"))); Sys.setenv(RVENV_UNSYNCED = Sys.getenv("RVENV")); cat(paste0("RIG_SWITCH_", "POISON_READY"))"#,
                "RIG_SWITCH_POISON_READY",
                PROMPT,
            )?;
            let checkpoint = terminal.checkpoint()?;
            terminal.enter(":switch! 4.5.3")?;
            let output = wait_for_replacement(terminal, checkpoint, "different-minor R switch")?;
            ensure!(
                output.contains("Project needs R 4.6, this is R 4.5."),
                "rig did not report the project R minor mismatch: {output}"
            );
            capture_proof(terminal, &script, &proof_dir, "switch-different-minor")?;

            let checkpoint = terminal.checkpoint()?;
            terminal.enter(":restart!")?;
            wait_for_replacement(terminal, checkpoint, "restart after different-minor switch")?;
            capture_proof(terminal, &script, &proof_dir, "restart-different-minor")?;
            terminal.quit()
        },
    )
}

#[test]
#[ignore = "requires rig 0.11.0 and synchronized project roots using R 4.6.1 and 4.6.0"]
fn rig_project_restart_reevaluates_the_current_working_directory() -> Result<()> {
    let state = state()?;
    let project = required_path(&state, "project")?;
    let project_b = required_path(&state, "project_b")?;
    let user_home = required_path(&state, "user_home")?;
    let script = required_path(&state, "proof_script")?;
    let proof_dir = required_path(&state, "proof_dir")?;
    let startup_dyld = state["base_dyld_library_path"]
        .as_str()
        .context("initial DYLD_LIBRARY_PATH in switch state")?;
    fs::create_dir_all(&proof_dir)?;

    run_case_with(
        Terminal::builder("rig-project-cwd-restart")
            .cwd(&project)
            .vanilla(false)
            .artifacts(proof_dir.join("cwd-restart-tui"))
            .env("HOME", user_home.to_string_lossy())
            .env("DYLD_LIBRARY_PATH", startup_dyld)
            .args([
                "--r-provider",
                "rig-project",
                "--no-r-auto-discovery",
                "--no-auto-match",
                "--no-completion",
                "--with-ipc",
                "--ipc-eval-unrestricted",
            ]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            capture_proof(terminal, &script, &proof_dir, "cwd-restart-project-a")?;

            let project_b = serde_json::to_string(&project_b.to_string_lossy())?;
            terminal.submit(
                &format!(r#"setwd({project_b}); cat("RIG_PROJECT_B_CWD_READY")"#),
                "RIG_PROJECT_B_CWD_READY",
                PROMPT,
            )?;
            capture_proof(
                terminal,
                &script,
                &proof_dir,
                "cwd-restart-before-project-b",
            )?;

            let checkpoint = terminal.checkpoint()?;
            terminal.enter(":restart!")?;
            wait_for_replacement(terminal, checkpoint, "restart in project B")?;
            capture_proof(terminal, &script, &proof_dir, "project-b-restart")?;
            terminal.quit()
        },
    )
}
