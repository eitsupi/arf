use super::terminal::output_events;
use super::{DEFAULT_CONFIG, PROMPT, SharedResources, Terminal, run_case_with};
use anyhow::{Context, Result, ensure};
use std::fs;

#[test]
fn recording_concatenates_output_without_input_or_resize_events() -> Result<()> {
    let recording = r#"{"version":3}
[0,"o","日"]
[0,"i","secret"]
[0,"r","80x24"]
[0,"o","本語\r\u001b[2K"]
"#;
    ensure!(output_events(recording)? == "日本語\r\x1b[2K");
    Ok(())
}

#[test]
fn fragmented_history_recording_does_not_contain_visual_result() -> Result<()> {
    // Reduced from a Windows ConPTY artifact: the corresponding saved screen
    // state displayed `[1] 3`, but this reduced raw recording does not contain
    // that visual result as a contiguous substring.
    let recording = r#"{"version":3}
[0,"o","[1] 2\r\n"]
[0,"o","\u001b[2K\rARF> "]
[0,"o","\u001b[2K\r[1]\r\u001b[3C 3\r\nARF> "]
"#;
    let output = output_events(recording)?;
    ensure!(
        output.matches("[1] 2").count() == 1,
        "initial result missing from fragmented recording: {output:?}"
    );
    ensure!(
        output.contains("\r\x1b[3C"),
        "cursor-control boundary missing from fragmented recording: {output:?}"
    );
    ensure!(
        !output.contains("[1] 3"),
        "raw recording unexpectedly contains the visual replay result: {output:?}"
    );
    Ok(())
}

#[test]
fn malformed_recordings_fail_instead_of_dropping_output() {
    for recording in [
        "",
        r#"{}
[0,"o",123]
"#,
        r#"{}
[0,"o"]
"#,
        r#"{}
truncated"#,
    ] {
        assert!(output_events(recording).is_err(), "accepted {recording:?}");
    }
}

#[test]
fn builder_applies_process_settings_and_shares_ipc_resources() -> Result<()> {
    let cwd = tempfile::tempdir()?;
    let history = tempfile::tempdir()?;
    let artifacts = tempfile::tempdir()?;
    let second_artifacts = tempfile::tempdir()?;
    let current_dir = std::env::current_dir()?;
    let config_file = tempfile::Builder::new()
        .prefix("arf-tui-config-")
        .tempfile_in(&current_dir)?;
    fs::write(
        config_file.path(),
        "[startup]\nshow_banner = false\n[prompt]\nformat = 'PATH> '",
    )?;
    let config_relative = config_file
        .path()
        .file_name()
        .context("temporary config has no file name")?
        .to_owned();
    let resources = SharedResources::new()?;
    let cwd_text = cwd.path().to_string_lossy().replace('\\', "/");
    let shared_sessions = resources.sessions_dir().to_path_buf();

    run_case_with(
        Terminal::builder("builder-settings")
            .config("[startup]\nshow_banner = false\n".to_owned() + DEFAULT_CONFIG)
            .env("ARF_BUILDER_ENV", "configured")
            .cwd(cwd.path())
            .cols(90)
            .rows(20)
            .history_dir(history.path())
            .artifacts(artifacts.path())
            .resources(resources.clone()),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            let state = terminal.state()?;
            ensure!(
                (state.cols, state.rows) == (90, 20),
                "wrong size: {state:?}"
            );
            ensure!(terminal.pid().is_some(), "missing arf PID");
            ensure!(
                terminal.sessions_dir() == shared_sessions,
                "wrong shared IPC directory"
            );
            ensure!(
                !state.text.contains("# arf console v"),
                "custom config did not disable the banner"
            );
            terminal.submit(
                "Sys.getenv('ARF_BUILDER_ENV')",
                r#"[1] "configured""#,
                PROMPT,
            )?;
            terminal.submit(
                &format!(
                    "normalizePath(getwd(), winslash='/') == normalizePath('{cwd_text}', winslash='/')"
                ),
                "[1] TRUE",
                PROMPT,
            )?;
            terminal.quit()
        },
    )?;
    ensure!(
        fs::read_dir(artifacts.path())?.next().is_some(),
        "recording artifacts were not retained"
    );
    ensure!(
        history.path().join("r.db").is_file(),
        "custom history directory did not contain r.db"
    );

    run_case_with(
        Terminal::builder("builder-shared-resource")
            .config_path(config_relative)
            .artifacts(second_artifacts.path())
            .resources(resources),
        |terminal| {
            terminal.wait_for_prompt(None, "PATH>")?;
            ensure!(
                terminal.sessions_dir() == shared_sessions,
                "second session did not reuse shared IPC directory"
            );
            terminal.quit()
        },
    )
}
