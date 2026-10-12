use super::support::{PROMPT, Terminal, build_formatter_fixture_path, run_case, run_case_with};
use anyhow::{Context, Result, ensure};

#[test]
fn menu_prompt_is_distinct_until_selection() -> Result<()> {
    run_case("menu-prompt", &["--no-auto-match"], |terminal| {
        terminal.enter("menu(c('option1', 'option2', 'option3'))")?;
        terminal.wait_for("menu selection prompt", |state, line| {
            state.text.contains("1: option1")
                && state.text.contains("2: option2")
                && state.text.contains("3: option3")
                && line.trim_end() == "Selection:"
        })?;
        terminal.write("2")?;
        terminal.key("Enter")?;
        terminal.wait_for_prompt(Some("[1] 2"), PROMPT)?;
        terminal.submit("1 + 1", "[1] 2", PROMPT)
    })
}

#[test]
fn custom_config_continuation_prompt_round_trips_via_raw_string() -> Result<()> {
    let config = r#"
[prompt]
format = "MAIN> "
continuation = "CONT> "
"#;
    run_case_with(
        Terminal::builder("custom-continuation")
            .args(["--no-auto-match"])
            .config(config),
        |terminal| {
            terminal.wait_for("custom main prompt", |_, line| line.trim_end() == "MAIN>")?;
            terminal.enter("options(continue = '... ')")?;
            terminal.wait_for("main prompt after options", |state, line| {
                state.text.contains("options(continue = '... ')") && line.trim_end() == "MAIN>"
            })?;
            terminal.write(r#"x <- r"(hello"#)?;
            terminal.key("Enter")?;
            terminal.wait_for("custom continuation prompt", |_, line| {
                line.trim_end() == "CONT>"
            })?;
            terminal.write(r#"world)""#)?;
            terminal.key("Enter")?;
            terminal.wait_for("custom main prompt after completion", |_, line| {
                line.trim_end() == "MAIN>"
            })?;
            terminal.submit("nchar(x)", "[1] 11", "MAIN>")?;
            terminal.quit()
        },
    )
}

#[test]
fn low_level_formatter_continuation_prompt_round_trips() -> Result<()> {
    for formatter in ["air", "arity"] {
        formatter_continuation_scenario(formatter)?;
    }
    Ok(())
}

fn formatter_continuation_scenario(formatter: &str) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = build_formatter_fixture_path(&temp.path().join("bin"), formatter)?;
    let config = r#"
[startup]
reprex = "format"

[reprex]
formatter = "__FORMATTER__"

[prompt]
format = "MAIN> "
continuation = "CONT> "
"#
    .replace("__FORMATTER__", formatter);
    run_case_with(
        Terminal::builder(format!("low-level-formatter-continuation-{formatter}"))
            .args(["--no-auto-match"])
            .config(config)
            .env("PATH", path)
            .env("FORMATTER_FIXTURE_MODE", "continuation"),
        |terminal| {
            terminal.wait_for("formatter main prompt", |_, line| {
                line.trim_end().ends_with("MAIN>")
            })?;
            terminal.enter("options(continue = '... ')")?;
            terminal.wait_for("formatter options prompt", |state, line| {
                state.text.contains("options(continue = '... ')")
                    && line.trim_end().ends_with("MAIN>")
            })?;

            terminal.enter("42")?;
            terminal.wait_for("formatter continuation prompt", |_, line| {
                line.trim_end().ends_with("CONT>")
            })?;
            terminal.write("2")?;
            terminal.key("Enter")?;
            terminal.wait_for("formatted expression result", |state, line| {
                state.text.contains("[1] 3") && line.trim_end().ends_with("MAIN>")
            })?;
            terminal.quit()
        },
    )
}

#[test]
fn identical_r_prompts_warn_once_per_transition() -> Result<()> {
    const WARNING: &str = r#"# [arf] Warning: options("prompt") and options("continue") are identical; arf cannot distinguish top-level and continuation prompts."#;

    const DISTINCT: &str = "options(prompt = '> ', continue = '+ ')";

    // Count warning rows on the emulated screen. Windows ConPTY can interleave
    // terminal mode sequences with the warning in the raw recording, so the
    // stream may not contain the warning as one substring. The session stays
    // short enough that no row scrolls away, and the terminal is wide enough
    // that the warning occupies a single row.
    fn warning_rows(text: &str) -> Vec<usize> {
        text.lines()
            .enumerate()
            .filter(|(_, line)| line.trim_end() == WARNING)
            .map(|(row, _)| row)
            .collect()
    }

    let builder = Terminal::builder("identical-r-prompts")
        .args(["--no-auto-match"])
        .cols(160);
    run_case_with(builder, |terminal| {
        terminal.wait_for_first_prompt()?;
        terminal.enter("options(prompt = '> ', continue = '> ')")?;
        terminal.wait_for("first ambiguity warning", |state, line| {
            !warning_rows(&state.text).is_empty() && line.trim_end() == "+"
        })?;
        let state = terminal.state()?;
        ensure!(
            warning_rows(&state.text).len() == 1,
            "expected one identical prompt warning: {}",
            state.text
        );

        terminal.enter("1 + 1")?;
        terminal.wait_for("ambiguous prompt persists", |state, line| {
            state.text.contains("[1] 2") && line.trim_end() == "+"
        })?;
        let state = terminal.state()?;
        ensure!(
            warning_rows(&state.text).len() == 1,
            "identical prompt warning repeated without a transition: {}",
            state.text
        );

        terminal.enter(DISTINCT)?;
        terminal.wait_for_prompt(None, PROMPT)?;
        terminal.enter("options(prompt = '> ', continue = '> ')")?;
        terminal.wait_for("second ambiguity warning", |state, line| {
            warning_rows(&state.text).len() > 1 && line.trim_end() == "+"
        })?;
        let state = terminal.state()?;
        let warnings = warning_rows(&state.text);
        let transition = state
            .text
            .lines()
            .position(|line| line.trim_end().ends_with(DISTINCT))
            .context("distinct prompt transition is missing from the screen")?;
        ensure!(
            matches!(warnings[..], [first, second] if first < transition && transition < second),
            "expected one warning before and one after the distinct prompt transition: {}",
            state.text
        );
        terminal.quit()
    })
}

#[test]
fn vi_mode_indicator_tracks_insert_and_normal_modes() -> Result<()> {
    let config = r#"
[editor]
mode = "vi"

[prompt]
format = "ARF> "

[prompt.vi]
symbol = { insert = "[I]", normal = "[N]" }
"#;
    run_case_with(
        Terminal::builder("vi-mode-indicator")
            .args(["--no-auto-match", "--no-completion"])
            .config(config),
        |terminal| {
            terminal.wait_for("vi insert indicator", |_, line| line.contains("ARF> [I]"))?;
            terminal.key("Escape")?;
            terminal.wait_for("vi normal indicator", |_, line| line.contains("ARF> [N]"))?;
            terminal.key("i")?;
            terminal.wait_for("vi insert indicator after i", |_, line| {
                line.contains("ARF> [I]")
            })?;
            terminal.quit()
        },
    )
}
