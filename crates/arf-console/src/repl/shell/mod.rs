//! Shell command execution and process management.

mod restart;

use std::io::{self, Write};
use std::process::{Command, Stdio};

use super::arf_eprintln;

pub use restart::restart_process;

/// Execute a shell command with direct stdin/stdout connection.
///
/// This uses inherited stdio so that:
/// - Interactive programs work (vim, less, python REPL)
/// - Commands that read stdin work (cat, read)
/// - Output streams in real-time
pub fn execute_shell_command(cmd: &str) {
    #[cfg(unix)]
    let result = {
        // Use user's default shell from $SHELL, fall back to /bin/sh
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        Command::new(&shell)
            .arg("-c")
            .arg(cmd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
    };

    #[cfg(windows)]
    let result = Command::new("cmd")
        .arg("/c")
        .arg(cmd)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn();

    match result {
        Ok(mut child) => {
            // Wait for the child process to complete
            if let Err(e) = child.wait() {
                arf_eprintln!("Failed to wait for command: {}", e);
            }
        }
        Err(e) => {
            arf_eprintln!("Failed to execute command: {}", e);
        }
    }
}

/// Prompt the user for confirmation (y/n).
///
/// Returns true if the user confirms, false otherwise.
pub fn confirm_action(prompt: &str) -> bool {
    print!("{} [y/N]: ", prompt);
    let _ = io::stdout().flush();

    let mut input = String::new();
    match io::stdin().read_line(&mut input) {
        Ok(_) => {
            let response = input.trim().to_lowercase();
            response == "y" || response == "yes"
        }
        Err(_) => false,
    }
}
