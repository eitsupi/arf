#![cfg(unix)]

use super::support::{PROMPT, Terminal};
use anyhow::Result;
use std::env;

#[test]
fn interactive_startup_reports_deprecated_config_warning_once_after_reexec() -> Result<()> {
    let Ok(library) = arf_libr::find_r_library() else {
        return Ok(());
    };
    let library_dir = library.parent().expect("R library directory");
    let r_home = library_dir
        .parent()
        .expect("R_HOME directory")
        .to_path_buf();
    let current_library_path = env::var_os("LD_LIBRARY_PATH").unwrap_or_default();
    let paths = env::split_paths(&current_library_path)
        .filter(|path| path != library_dir)
        .collect::<Vec<_>>();
    let library_path = env::join_paths(paths).expect("valid LD_LIBRARY_PATH");
    assert!(
        !env::split_paths(&library_path).any(|path| path == library_dir),
        "test must force the loader re-exec"
    );

    let config = r#"[history]
disabled = true

[prompt]
format = '{status}ARF> '

[prompt.status.symbol]
error = 'ERR '
"#;
    let terminal = Terminal::builder("deprecated-config-warning")
        .config(config)
        .env("R_HOME", r_home.to_string_lossy())
        .env("LD_LIBRARY_PATH", library_path.to_string_lossy())
        .env("RUST_LOG", "warn")
        .args(["--no-banner", "--no-auto-match", "--no-completion"])
        .spawn()?;

    terminal.wait_for_prompt(None, PROMPT)?;
    let output = terminal.output()?;
    let warning_lines = output
        .lines()
        .filter(|line| line.contains("Warning: Config key history.disabled"))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n");
    insta::assert_snapshot!(warning_lines, @r###"Warning: Config key history.disabled is deprecated; use history.mode = "volatile" instead."###);
    terminal.quit()?;
    Ok(())
}

#[test]
fn interactive_startup_uses_arf_config_and_reports_its_path() -> Result<()> {
    let config_dir = tempfile::tempdir()?;
    let config_path = config_dir.path().join("interactive.toml");
    std::fs::write(
        &config_path,
        r#"[prompt]
format = '{status}ENV> '
"#,
    )?;
    let directory_name = config_dir
        .path()
        .file_name()
        .expect("temporary config directory name")
        .to_string_lossy();
    let expected_path_suffix = format!("{directory_name}/interactive.toml");
    let terminal = Terminal::builder("config-env")
        .config_path(&config_path)
        .config_from_env()
        .args(["--no-banner", "--no-auto-match", "--no-completion"])
        .spawn()?;

    terminal.wait_for_prompt(None, "ENV>")?;
    terminal.submit(
        "Sys.unsetenv('ARF_CONFIG'); cat('ENV_CHANGED\\n')",
        "ENV_CHANGED",
        "ENV>",
    )?;
    for command in [":info", ":session"] {
        terminal.enter(command)?;
        terminal.wait_for("ARF_CONFIG source and effective path", |state, _| {
            state.text.contains("Config source:  ARF_CONFIG")
                && state.text.lines().any(|line| {
                    line.contains("Config file:")
                        && line.replace('\\', "/").contains(&expected_path_suffix)
                })
        })?;
        terminal.key("q")?;
        terminal.wait_for_prompt(None, "ENV>")?;
    }
    terminal.quit()?;
    Ok(())
}
