//! Working directory meta commands (`:cd`, `:pushd`, `:popd`).

use crate::completion::path::expand_tilde;
use std::path::PathBuf;

/// Change the current working directory.
///
/// If `path_arg` is empty, changes to the home directory.
/// Tilde (`~`) is expanded to the home directory.
pub(super) fn meta_cd(path_arg: &str) -> Result<PathBuf, String> {
    let target = if path_arg.is_empty() {
        dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?
    } else {
        PathBuf::from(expand_tilde(path_arg))
    };
    std::env::set_current_dir(&target).map_err(|e| format!("{}: {}", target.display(), e))?;
    std::env::current_dir().map_err(|e| e.to_string())
}

/// Push the current directory onto the stack and change to a new directory.
///
/// Requires a path argument. Unlike bash's `pushd` (which swaps the top two
/// stack entries when called without arguments), this always requires an
/// explicit destination.
pub(super) fn meta_pushd(dir_stack: &mut Vec<PathBuf>, path_arg: &str) -> Result<PathBuf, String> {
    if path_arg.is_empty() {
        return Err("Usage: :pushd <path>".to_string());
    }
    let current = std::env::current_dir().map_err(|e| e.to_string())?;
    let new_dir = meta_cd(path_arg)?;
    dir_stack.push(current);
    Ok(new_dir)
}

/// Pop the top directory from the stack and change to it.
pub(super) fn meta_popd(dir_stack: &mut Vec<PathBuf>) -> Result<PathBuf, String> {
    let target = dir_stack
        .last()
        .cloned()
        .ok_or_else(|| "Directory stack is empty".to_string())?;
    std::env::set_current_dir(&target).map_err(|e| format!("{}", e))?;
    dir_stack.pop();
    std::env::current_dir().map_err(|e| e.to_string())
}

/// Return a hint message if the shell command is a directory navigation command
/// that won't work as expected in a subprocess.
pub(in crate::repl) fn dir_command_hint(shell_cmd: &str) -> Option<&'static str> {
    match shell_cmd.split_whitespace().next()? {
        "cd" => Some("Hint: Use the :cd meta command instead to change directory."),
        "pushd" => Some("Hint: Use the :pushd meta command instead to change directory."),
        "popd" => Some("Hint: Use the :popd meta command instead to restore directory."),
        _ => None,
    }
}
