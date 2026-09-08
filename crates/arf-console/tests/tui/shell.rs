use super::support::{ERROR_PROMPT, PROMPT, Terminal, run_case, run_case_with};
use anyhow::Result;

const SHELL_CONFIG: &str = r#"
[prompt]
format = '{status}ARF> '
[prompt.status.symbol]
error = 'ERR '

[experimental]
shell_semicolon_shortcut = true

[experimental.shell_abbreviations]
"abbr_test" = "echo ABBR_EXPANDED"
"abbr_sp" = "echo SPACE_EXPANDED"
"#;

fn shell_prompt(line: &str) -> bool {
    let line = line.trim_end();
    // The prompt format is shared across platforms.  Only the shell name
    // differs (`[bash] $` on POSIX versus `[cmd] $` on Windows), so keep the
    // adapter narrow enough that an arbitrary output line cannot be mistaken
    // for readiness.
    line.contains("] $")
}

fn enter_shell(terminal: &Terminal) -> Result<()> {
    terminal.enter(":shell")?;
    terminal.wait_for("shell mode enabled", |state, line| {
        state.text.contains("Shell mode enabled") && shell_prompt(line)
    })
}

fn leave_shell(terminal: &Terminal) -> Result<()> {
    // Send the meta command as two observable editor events.  In particular,
    // a semicolon shortcut enters shell mode from an editor callback; keeping
    // typing and submission separate avoids coalescing `:r` with that
    // transition on slower terminal backends.
    terminal.write(":r")?;
    terminal.wait_for("shell return command input", |_, line| {
        shell_prompt(line) && line.contains(":r")
    })?;
    terminal.write("\r")?;
    terminal.wait_for("returned to R mode", |state, line| {
        state.text.contains("Returned to R mode")
            && state.session_shell.is_none()
            && (line.trim_end() == PROMPT || line.contains(":r"))
    })
}

#[test]
fn readline_accepts_input_and_returns_to_r_prompt() -> Result<()> {
    run_case("readline", &["--no-auto-match"], |terminal| {
        terminal.enter("cat('READLINE_HELLO'); readline('input> ')")?;
        terminal.wait_for("readline prompt", |state, line| {
            state.exited.is_none()
                && state.text.contains("READLINE_HELLO")
                && line.contains("input> ")
        })?;
        terminal.enter("readline_answer")?;
        terminal.wait_for("readline result", |state, line| {
            state
                .text
                .lines()
                .any(|line| line.contains("\"readline_answer\""))
                && line.trim_end() == PROMPT
        })
    })
}

#[test]
fn shell_mode_runs_commands_and_returns_to_r() -> Result<()> {
    run_case(
        "shell-mode",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            enter_shell(terminal)?;
            terminal.enter("echo SHELL_MODE_OUTPUT")?;
            terminal.wait_for("shell command output", |state, line| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim_end() == "SHELL_MODE_OUTPUT")
                    && shell_prompt(line)
            })?;
            leave_shell(terminal)?;
            terminal.submit("42", "[1] 42", PROMPT)
        },
    )
}

#[test]
fn system_command_runs_without_leaving_r_mode() -> Result<()> {
    run_case(
        "system-command",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            terminal.enter(":system echo SYSTEM_COMMAND_OUTPUT")?;
            terminal.wait_for("system command output", |state, line| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim_end() == "SYSTEM_COMMAND_OUTPUT")
                    && line.trim_end() == PROMPT
            })?;
            terminal.submit("100", "[1] 100", PROMPT)
        },
    )
}

#[test]
fn ctrl_c_leaves_shell_mode_and_r_recovers() -> Result<()> {
    run_case(
        "shell-mode-ctrl-c",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            enter_shell(terminal)?;
            terminal.key("Ctrl+C")?;
            terminal.wait_for("shell Ctrl+C returns to R", |state, line| {
                state.text.contains("Returned to R mode") && line.trim_end() == PROMPT
            })?;
            terminal.submit("200", "[1] 200", PROMPT)
        },
    )
}

#[test]
fn semicolon_shortcut_enters_shell_mode() -> Result<()> {
    run_case_with(
        Terminal::builder("semicolon-shell").config(SHELL_CONFIG),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.write(";")?;
            terminal.wait_for("semicolon enters shell mode", |state, line| {
                state.text.contains("Shell mode enabled") && shell_prompt(line)
            })?;
            terminal.enter("echo SEMICOLON_SHELL_OUTPUT")?;
            terminal.wait_for("semicolon shell output", |state, line| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim_end() == "SEMICOLON_SHELL_OUTPUT")
                    && shell_prompt(line)
            })?;
            // The shortcut is itself a keybinding transition.  Ctrl+C is the
            // shell editor's direct return path and also avoids carrying a
            // partially submitted `:r` buffer across that transition.
            terminal.key("Ctrl+C")?;
            terminal.wait_for("semicolon shell returns to R", |state, line| {
                state.text.contains("Returned to R mode") && line.trim_end() == PROMPT
            })?;
            terminal.quit()
        },
    )
}

#[test]
fn semicolon_inside_r_expression_stays_literal() -> Result<()> {
    run_case_with(
        Terminal::builder("semicolon-expression").config(SHELL_CONFIG),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.write("1")?;
            terminal.wait_for("first semicolon expression character", |_, line| {
                line.trim_end() == "ARF> 1"
            })?;
            terminal.write(";")?;
            terminal.wait_for("literal semicolon", |_, line| line.trim_end() == "ARF> 1;")?;
            terminal.enter("2")?;
            terminal.wait_for_prompt(Some("[1] 2"), PROMPT)?;
            terminal.quit()
        },
    )
}

#[test]
fn shell_abbreviation_expands_on_enter() -> Result<()> {
    run_case_with(
        Terminal::builder("shell-abbreviation-enter")
            .config(SHELL_CONFIG)
            .args(["--no-auto-match", "--no-completion"]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            enter_shell(terminal)?;
            terminal.write("abbr_test")?;
            terminal.wait_for("abbreviation input", |_, line| {
                shell_prompt(line) && line.contains("abbr_test")
            })?;
            terminal.key("Enter")?;
            terminal.wait_for("abbreviation output", |state, line| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim_end() == "ABBR_EXPANDED")
                    && shell_prompt(line)
            })?;
            leave_shell(terminal)?;
            terminal.quit()
        },
    )
}

#[test]
fn shell_abbreviation_is_not_active_in_r_mode() -> Result<()> {
    run_case_with(
        Terminal::builder("shell-abbreviation-r")
            .config(SHELL_CONFIG)
            .args(["--no-auto-match", "--no-completion"]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            terminal.enter("abbr_test")?;
            terminal.wait_for("R abbreviation error", |state, line| {
                state.text.contains("object 'abbr_test' not found")
                    && line.trim_end() == ERROR_PROMPT
            })?;
            terminal.quit()
        },
    )
}

#[test]
fn shell_abbreviation_expands_on_space() -> Result<()> {
    run_case_with(
        Terminal::builder("shell-abbreviation-space")
            .config(SHELL_CONFIG)
            .args(["--no-auto-match", "--no-completion"]),
        |terminal| {
            terminal.wait_for_first_prompt()?;
            enter_shell(terminal)?;
            terminal.write("abbr_sp")?;
            terminal.wait_for("space abbreviation input", |_, line| {
                shell_prompt(line) && line.contains("abbr_sp")
            })?;
            terminal.write(" ")?;
            terminal.wait_for("space abbreviation expansion", |_, line| {
                shell_prompt(line) && line.contains("echo SPACE_EXPANDED")
            })?;
            terminal.key("Enter")?;
            terminal.wait_for("space abbreviation output", |state, line| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim_end() == "SPACE_EXPANDED")
                    && shell_prompt(line)
            })?;
            leave_shell(terminal)?;
            terminal.quit()
        },
    )
}
