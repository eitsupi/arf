use super::support::{ERROR_PROMPT, PROMPT, Terminal, run_case, run_case_with};
use anyhow::{Result, ensure};

fn bracketed_paste(terminal: &Terminal, content: &str) -> Result<()> {
    let marker = format!("PASTE_DONE_{}", content.len());
    terminal.write(&format!(
        "\x1b[200~{content}\ncat('{marker}\\n')\x1b[201~\r"
    ))?;
    terminal.wait_for_prompt(Some(&marker), PROMPT)
}

#[test]
fn arrow_and_backspace_edit_input_before_evaluation() -> Result<()> {
    run_case("editing", &[], |terminal| {
        terminal.write("1 + 40")?;
        terminal.wait_for("input echo", |_, line| line.trim_end() == "ARF> 1 + 40")?;
        terminal.key("Left")?;
        terminal.key("Backspace")?;
        terminal.write("2")?;
        terminal.wait_for("edited input", |_, line| line.trim_end() == "ARF> 1 + 20")?;
        terminal.key("Enter")?;
        terminal.wait_for_prompt(Some("[1] 21"), PROMPT)
    })
}

#[test]
fn ctrl_c_discards_input_and_allows_a_new_command() -> Result<()> {
    run_case("cancel-input", &[], |terminal| {
        terminal.write("unfinished_input")?;
        terminal.wait_for("input before cancellation", |_, line| {
            line.trim_end() == "ARF> unfinished_input"
        })?;
        terminal.key("Ctrl+C")?;
        terminal.wait_for_prompt(None, PROMPT)?;
        terminal.submit(
            r"Sys.sleep(0.4); cat(paste0('CANCEL_', 'OK'), '\n')",
            "CANCEL_OK",
            PROMPT,
        )
    })
}

#[test]
fn ctrl_c_interrupts_running_command_and_allows_recovery() -> Result<()> {
    run_case("interrupt-computation", &["--no-auto-match"], |terminal| {
        terminal.enter("cat('SLEEP_START\\n'); Sys.sleep(30)")?;
        terminal.wait_for("long command started", |state, _| {
            state.exited.is_none() && state.text.contains("SLEEP_START")
        })?;
        terminal.key("Ctrl+C")?;
        terminal.wait_for("prompt after interrupt", |state, line| {
            state.exited.is_none() && matches!(line.trim_end(), PROMPT | ERROR_PROMPT)
        })?;
        terminal.submit("42", "[1] 42", PROMPT)
    })
}

#[test]
fn ctrl_d_with_content_does_not_exit_or_drop_input() -> Result<()> {
    run_case(
        "ctrl-d-content",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            terminal.write("abc")?;
            terminal.wait_for("input before Ctrl+D", |_, line| {
                line.trim_end() == "ARF> abc"
            })?;
            terminal.key("Ctrl+D")?;
            terminal.wait_for("input survives Ctrl+D", |state, line| {
                state.exited.is_none() && line.trim_end() == "ARF> abc"
            })?;
            terminal.key("Ctrl+C")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.submit("42", "[1] 42", PROMPT)
        },
    )
}

#[test]
fn cursor_position_tracks_empty_input_and_interrupt() -> Result<()> {
    run_case(
        "cursor-position",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            let initial = terminal.state()?;
            let initial_row = initial.cursor.y;
            let initial_col = initial.cursor.x;
            ensure!(initial_col > 0, "cursor should start after the prompt");

            terminal.key("Enter")?;
            terminal.wait_for("empty input moves to a new prompt", |state, line| {
                state.cursor.y > initial_row
                    && state.cursor.x == initial_col
                    && line.trim_end() == PROMPT
            })?;

            terminal.write("a")?;
            terminal.wait_for("typed input advances the cursor", |state, line| {
                state.cursor.x == initial_col + 1 && line.trim_end() == "ARF> a"
            })?;

            terminal.key("Ctrl+C")?;
            terminal.wait_for("interrupt restores prompt position", |state, line| {
                state.cursor.x == initial_col && line.trim_end() == PROMPT
            })
        },
    )
}

#[test]
fn screen_state_tracks_results_and_checkpointed_output() -> Result<()> {
    run_case(
        "screen-state",
        &["--no-auto-match", "--no-completion"],
        |terminal| {
            let initial = terminal.state()?;
            let initial_row = initial.cursor.y;
            ensure!(initial.cursor.x > 0, "cursor should start after the prompt");
            ensure!(initial.text.lines().any(|line| line.trim_end() == PROMPT));

            terminal.submit("100", "[1] 100", PROMPT)?;
            let after_first = terminal.state()?;
            ensure!(after_first.cursor.y > initial_row);
            ensure!(after_first.text.contains("[1] 100"));
            ensure!(after_first.cursor.x == initial.cursor.x);

            // The checkpoint replaces the legacy PTY output-buffer clearing
            // while retaining the observable result of the second command.
            let checkpoint = terminal.checkpoint()?;
            terminal.submit("200", "[1] 200", PROMPT)?;
            ensure!(
                terminal.output_since(checkpoint)?.contains("[1] 200"),
                "second result was not emitted after the checkpoint"
            );
            Ok(())
        },
    )
}

#[test]
fn backtick_error_does_not_crash_and_repl_recovers() -> Result<()> {
    run_case("backtick-error", &["--no-completion"], |terminal| {
        terminal.write("`")?;
        terminal.wait_for("auto-matched quote", |state, line| {
            state.exited.is_none() && line.contains("``")
        })?;
        terminal.key("Enter")?;
        terminal.wait_for("backtick parser error", |state, _| {
            state.text.contains("zero-length variable name")
        })?;
        terminal.wait_for("prompt after backtick error", |_, line| {
            line.trim_end() == ERROR_PROMPT
        })?;
        terminal.submit("1 + 1", "[1] 2", PROMPT)
    })
}

#[test]
fn multiline_raw_string_round_trip() -> Result<()> {
    run_case("multiline-raw-string", &["--no-auto-match"], |terminal| {
        terminal.write(r#"x <- r"(hello"#)?;
        terminal.key("Enter")?;
        terminal.wait_for("raw string continuation", |_, line| line.trim_end() == "+")?;
        terminal.write(r#"world)""#)?;
        terminal.key("Enter")?;
        terminal.wait_for_prompt(None, PROMPT)?;
        terminal.submit("nchar(x)", "[1] 11", PROMPT)
    })
}

#[test]
fn multiline_quoted_string_preserves_newline() -> Result<()> {
    run_case(
        "multiline-quoted-string",
        &["--no-auto-match"],
        |terminal| {
            terminal.write(r#"x <- "test"#)?;
            terminal.key("Enter")?;
            terminal.wait_for("quoted string continuation", |_, line| {
                line.trim_end() == "+"
            })?;
            terminal.write(r#"end""#)?;
            terminal.key("Enter")?;
            terminal.wait_for_prompt(None, PROMPT)?;
            terminal.submit("nchar(x)", "[1] 8", PROMPT)?;
            terminal.submit(r#"identical(x, "test\nend")"#, "[1] TRUE", PROMPT)
        },
    )
}

#[test]
#[ignore = "requires PR #330 reedline auto-pairs"]
fn raw_string_with_auto_match_is_preserved() -> Result<()> {
    run_case("raw-string-auto-match", &[], |terminal| {
        let source = r#"x <- r"---(hello "world")---"#;
        terminal.write(source)?;
        terminal.wait_for("raw string source is constructed", |_, line| {
            line.contains(source)
        })?;
        terminal.key("Enter")?;
        terminal.wait_for("raw string assignment completed", |state, line| {
            state.text.contains(source) && line.trim_end() == PROMPT
        })?;
        terminal.submit("nchar(x)", "[1] 13", PROMPT)
    })
}

#[test]
fn bracketed_paste_handles_basic_long_multiline_and_multibyte_text() -> Result<()> {
    run_case("bracketed-paste-shapes", &["--no-auto-match"], |terminal| {
        bracketed_paste(terminal, "basic <- 'abcdefghij'")?;
        terminal.submit("nchar(basic)", "[1] 10", PROMPT)?;

        bracketed_paste(terminal, &format!("long <- '{}'", "a".repeat(5000)))?;
        terminal.submit("nchar(long)", "[1] 5000", PROMPT)?;

        bracketed_paste(
            terminal,
            &format!("multiline <- '{}\\n{}'", "a".repeat(2000), "b".repeat(2000)),
        )?;
        terminal.submit("nchar(multiline)", "[1] 4001", PROMPT)?;

        let multibyte = "中".repeat(1000)
            + "\\n"
            + &"文".repeat(1000)
            + "\\n"
            + &"中".repeat(1000)
            + "\\n"
            + &"文".repeat(1000);
        bracketed_paste(terminal, &format!("x <- '{multibyte}'"))?;
        terminal.submit("nchar(x)", "[1] 4003", PROMPT)?;
        bracketed_paste(terminal, &format!("xy <- '{multibyte}'"))?;
        terminal.submit("nchar(xy)", "[1] 4003", PROMPT)
    })
}

#[test]
fn bracketed_paste_executes_multiple_expressions() -> Result<()> {
    run_case(
        "bracketed-paste-multiple",
        &["--no-auto-match"],
        |terminal| {
            terminal.write("\x1b[200~1 + 1\n2 + 2\ncat('PASTE_DONE_MULTIPLE\\n')\x1b[201~\r")?;
            terminal.wait_for("both pasted expressions", |state, _| {
                state.text.contains("[1] 2") && state.text.contains("[1] 4")
            })?;
            terminal.wait_for_prompt(Some("PASTE_DONE_MULTIPLE"), PROMPT)
        },
    )
}

#[test]
fn bracketed_paste_does_not_duplicate_auto_matched_brackets() -> Result<()> {
    run_case("bracketed-paste-auto-match", &[], |terminal| {
        bracketed_paste(terminal, "x <- (1)")?;
        terminal.submit("x", "[1] 1", PROMPT)?;
        bracketed_paste(terminal, "y <- sum(c(1, 2, 3))")?;
        terminal.submit("y", "[1] 6", PROMPT)
    })
}

#[test]
fn bracketed_paste_before_first_prompt_is_not_echoed() -> Result<()> {
    let profile = tempfile::tempdir()?;
    let profile_path = profile.path().join(".Rprofile");
    let release_dir = tempfile::tempdir()?;
    let release_path = release_dir.path().join("release");
    std::fs::write(
        &profile_path,
        "cat('PROFILE_MARKER\\n')\nwhile (!file.exists(Sys.getenv('ARF_TUI_RELEASE_FILE'))) Sys.sleep(0.01)\n",
    )?;
    run_case_with(
        Terminal::builder("bracketed-paste-before-prompt")
            .args(["--no-auto-match"])
            .vanilla(false)
            .env(
                "R_PROFILE_USER",
                profile_path.to_string_lossy().into_owned(),
            )
            .env(
                "ARF_TUI_RELEASE_FILE",
                release_path.to_string_lossy().into_owned(),
            ),
        |terminal| {
            // The profile marker is emitted before the blocking release-file
            // loop, which guarantees that the first prompt has not appeared.
            terminal.wait_for("profile marker before first prompt", |state, _| {
                state.exited.is_none() && state.text.contains("PROFILE_MARKER")
            })?;
            terminal.write("\x1b[200~x <- 42\x1b[201~\r")?;
            std::fs::write(&release_path, "release")?;
            terminal.wait_for("queued assignment and first prompt", |state, line| {
                state
                    .text
                    .lines()
                    .any(|line| line.trim_end() == "ARF> x <- 42")
                    && line.trim_end() == PROMPT
                    && usize::from(state.cursor.x) == PROMPT.len() + 1
            })?;
            let output = terminal.output()?;
            ensure!(
                !output.contains("\x1b[200~") && !output.contains("^[[200~"),
                "bracketed paste start was echoed before first prompt: {output:?}"
            );
            ensure!(
                !output.contains("\x1b[201~") && !output.contains("^[[201~"),
                "bracketed paste end was echoed before first prompt: {output:?}"
            );
            terminal.submit("x", "[1] 42", PROMPT)?;
            terminal.quit()
        },
    )
}

/// The startup guard must not remain the terminal mode baseline once reedline
/// has completed its first raw-mode read. Inspect the PTY from R evaluation so
/// this exercises the same terminal inherited by child processes.
#[cfg(unix)]
#[test]
fn cooked_terminal_mode_is_restored_during_evaluation() -> Result<()> {
    run_case("cooked-terminal-mode", &[], |terminal| {
        terminal.enter(
            r#"system("printf 'ARF_TERM_BEGIN_352\n'; stty -a 2>&1; printf 'ARF_TERM_END_352\n'")"#,
        )?;
        terminal.wait_for("cooked terminal mode", |state, line| {
            let Some(begin) = state.text.rfind("ARF_TERM_BEGIN_352") else {
                return false;
            };
            let Some(end) = state.text[begin..].find("ARF_TERM_END_352") else {
                return false;
            };
            let output = state.text[begin..begin + end].to_ascii_lowercase();
            let flags: Vec<_> = output.split_whitespace().collect();
            line.trim_end() == PROMPT
                && flags.contains(&"icanon")
                && flags.contains(&"echo")
                && !flags.contains(&"-icanon")
                && !flags.contains(&"-echo")
        })?;
        bracketed_paste(terminal, "cat('COOKED_PASTE_OK\\n')")?;
        terminal.submit("cat('COOKED_MODE_OK\\n')", "COOKED_MODE_OK", PROMPT)
    })
}

#[test]
fn multiline_function_uses_continuation_then_returns_to_prompt() -> Result<()> {
    run_case("multiline", &["--no-auto-match"], |terminal| {
        terminal.enter("f <- function(x) {")?;
        terminal.wait_for("continuation prompt", |_, line| line.trim_end() == "+")?;
        terminal.enter("x + 1")?;
        // Require the new body to be echoed, not just the preceding continuation.
        terminal.wait_for("function body and continuation", |state, line| {
            line.trim_end() == "+" && state.text.contains("x + 1")
        })?;
        terminal.enter("}")?;
        terminal.wait_for_prompt(None, PROMPT)?;
        terminal.submit("f(10)", "[1] 11", PROMPT)
    })
}

#[test]
fn unicode_output_preserves_wide_and_combining_characters() -> Result<()> {
    run_case("unicode", &[], |terminal| {
        terminal.submit(
            r"cat(intToUtf8(c(26085, 26412, 35486)), '\n')",
            "日本語",
            PROMPT,
        )?;
        terminal.submit(r"cat(intToUtf8(c(101, 769)), '\n')", "e\u{301}", PROMPT)
    })
}
