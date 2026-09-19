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
    assert_eq!(
        output
            .matches("Warning: Config key history.disabled")
            .count(),
        1,
        "{output}"
    );
    terminal.quit()?;
    Ok(())
}
