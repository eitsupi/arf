use super::support::{PROMPT, Terminal, run_case, run_case_with};
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
            let options_checkpoint = terminal.checkpoint()?;
            terminal.enter("options(continue = '... ')")?;
            terminal.wait_for("main prompt after options", |state, line| {
                state.text.contains("options(continue = '... ')") && line.trim_end() == "MAIN>"
            })?;
            ensure!(
                terminal
                    .output_since(options_checkpoint)?
                    .contains("options(continue ")
            );
            terminal.write("x <- r\"(hello")?;
            terminal.key("Enter")?;
            terminal.wait_for("custom continuation prompt", |_, line| {
                line.trim_end() == "CONT>"
            })?;
            terminal.write("world)\"")?;
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
#[cfg(unix)]
fn low_level_formatter_continuation_prompt_round_trips() -> Result<()> {
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
      printf '%s' '1 +'
    else
      printf '%s' "$input"
    fi
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
    let config = r#"
[startup]
reprex = "format"

[reprex]
formatter = "arity"

[prompt]
format = "MAIN> "
continuation = "CONT> "
"#;
    run_case_with(
        Terminal::builder("low-level-formatter-continuation")
            .args(["--no-auto-match"])
            .config(config)
            .env("PATH", path),
        |terminal| {
            terminal.wait_for("formatter main prompt", |_, line| {
                line.trim_end().ends_with("MAIN>")
            })?;
            let options_checkpoint = terminal.checkpoint()?;
            terminal.enter("options(continue = '... ')")?;
            terminal.wait_for("formatter options prompt", |state, line| {
                state.text.contains("options(continue = '... ')")
                    && line.trim_end().ends_with("MAIN>")
            })?;
            ensure!(
                terminal
                    .output_since(options_checkpoint)?
                    .contains("options(continue ")
            );

            let formatted_checkpoint = terminal.checkpoint()?;
            terminal.enter("42")?;
            terminal.wait_for("formatter continuation prompt", |_, line| {
                line.trim_end().ends_with("CONT>")
            })?;
            terminal.write("2")?;
            terminal.key("Enter")?;
            terminal.wait_for("formatted expression result", |state, line| {
                state.text.contains("[1] 3") && line.trim_end().ends_with("MAIN>")
            })?;
            ensure!(
                terminal
                    .output_since(formatted_checkpoint)?
                    .contains("[1] 3")
            );
            terminal.quit()
        },
    )
}

#[test]
fn identical_r_prompts_warn_once_per_transition() -> Result<()> {
    const WARNING: &str = "# [arf] Warning: options(\"prompt\") and options(\"continue\") are identical; arf cannot distinguish top-level and continuation prompts.";

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
