use super::support::{PROMPT, Terminal, build_formatter_fixture_path, run_case, run_case_with};
use anyhow::{Result, ensure};

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

#[test]
fn formatter_evaluates_complete_prefix_before_continuation_input() -> Result<()> {
    for formatter in ["air", "arity"] {
        formatter_prefix_continuation_scenario(formatter)?;
    }
    Ok(())
}

#[test]
fn formatter_continuation_cancellation_discards_parent_and_recovers() -> Result<()> {
    for formatter in ["air", "arity"] {
        formatter_continuation_cancellation_scenario(formatter)?;
    }
    Ok(())
}

fn formatter_prefix_continuation_scenario(formatter: &str) -> Result<()> {
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
        Terminal::builder(format!("formatter-prefix-continuation-{formatter}"))
            .args(["--no-auto-match"])
            .config(config)
            .env("PATH", path)
            .env("FORMATTER_FIXTURE_MODE", "prefix-continuation"),
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
            terminal.wait_for("prefix output before continuation input", |state, line| {
                state.text.lines().any(|line| line.trim() == "#> PREFIX")
                    && line.trim_end().ends_with("CONT>")
            })?;
            terminal.enter("2")?;
            terminal.wait_for("formatted result after continuation", |state, line| {
                state.text.lines().any(|line| line.trim() == "#> [1] 3")
                    && line.trim_end().ends_with("MAIN>")
            })?;
            terminal.quit()
        },
    )
}

fn formatter_continuation_cancellation_scenario(formatter: &str) -> Result<()> {
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
        Terminal::builder(format!("formatter-continuation-cancel-{formatter}"))
            .args(["--no-auto-match"])
            .config(config)
            .env("PATH", path)
            .env("FORMATTER_FIXTURE_MODE", "prefix-continuation"),
        |terminal| {
            terminal.wait_for("formatter main prompt", |_, line| {
                line.trim_end().ends_with("MAIN>")
            })?;
            terminal.enter("options(continue = '... ')")?;
            terminal.wait_for("formatter options prompt", |state, line| {
                state.text.contains("options(continue = '... ')")
                    && line.trim_end().ends_with("MAIN>")
            })?;

            let command_start = terminal.checkpoint()?;
            terminal.enter("42")?;
            terminal.wait_for("prefix output before continuation input", |state, line| {
                state.text.lines().any(|line| line.trim() == "#> PREFIX")
                    && line.trim_end().ends_with("CONT>")
            })?;
            terminal.key("Ctrl+C")?;
            terminal.wait_for("prompt after continuation cancellation", |state, line| {
                state.text.contains("^C") && line.trim_end().ends_with("MAIN>")
            })?;
            let cancelled_command = terminal.output_since(command_start)?;
            ensure!(
                cancelled_command.matches("#> PREFIX").count() == 1,
                "the completed formatter prefix should run once before cancellation"
            );

            terminal.enter("10 + 1")?;
            terminal.wait_for("next command after cancellation", |state, line| {
                state.text.lines().any(|line| line.trim() == "#> [1] 11")
                    && line.trim_end().ends_with("MAIN>")
            })?;
            terminal.quit()
        },
    )
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
            terminal.write("2)")?;
            terminal.key("Enter")?;
            terminal.wait_for("formatted expression result", |state, line| {
                state.text.contains("[1] 3") && line.trim_end().ends_with("MAIN>")
            })?;
            terminal.enter("10 + 1")?;
            terminal.wait_for("validator restored for next command", |state, line| {
                state.text.contains("[1] 11") && line.trim_end().ends_with("MAIN>")
            })?;

            terminal.quit()
        },
    )
}

#[test]
fn identical_r_prompts_warn_once_per_transition() -> Result<()> {
    const WARNING: &str = r#"# [arf] Warning: options("prompt") and options("continue") are identical; arf cannot distinguish top-level and continuation prompts."#;

    run_case("identical-r-prompts", &["--no-auto-match"], |terminal| {
        terminal.enter("options(prompt = '> ', continue = '> ')")?;
        terminal.wait_for("first ambiguity warning", |state, line| {
            state.text.contains("# [arf] Warning: options") && line.trim_end() == "+"
        })?;
        let first = terminal.output()?;
        ensure!(first.matches(WARNING).count() == 1);

        let persistent = terminal.checkpoint()?;
        terminal.enter("1 + 1")?;
        terminal.wait_for("ambiguous prompt persists", |state, line| {
            state.text.contains("[1] 2") && line.trim_end() == "+"
        })?;
        ensure!(
            !terminal.output_since(persistent)?.contains(WARNING),
            "identical prompt warning repeated without a transition"
        );

        terminal.enter("options(prompt = '> ', continue = '+ ')")?;
        terminal.wait_for_prompt(None, PROMPT)?;
        let distinct = terminal.checkpoint()?;
        terminal.enter("options(prompt = '> ', continue = '> ')")?;
        terminal.wait_for("second ambiguity warning", |state, line| {
            state.text.contains("# [arf] Warning: options") && line.trim_end() == "+"
        })?;
        ensure!(terminal.output_since(distinct)?.matches(WARNING).count() == 1);
        Ok(())
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
