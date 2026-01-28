//! Shell command execution and process management.

use crate::external::rig;
use std::io::{self, Write};
use std::process::{Command, Stdio};

use super::arf_eprintln;

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

/// Restart the process, optionally with a new R version.
///
/// This function uses exec() to replace the current process with a new instance.
/// If version is specified, it resolves the R_HOME using rig before restarting.
///
/// This function only returns if exec fails.
pub fn restart_process(version: Option<&str>) {
    // If a version is specified, validate it using rig before restarting
    if let Some(ver) = version {
        if !rig::rig_available() {
            arf_eprintln!("Error: rig is not installed. Cannot switch R versions.");
            arf_eprintln!("Install rig from https://github.com/r-lib/rig");
            return;
        }

        // Validate the version exists before restarting
        match rig::resolve_version(ver) {
            Ok(resolved) => {
                log::info!("Switching to R version {} ({})", resolved.version, resolved.r_home);
            }
            Err(e) => {
                arf_eprintln!("Error: {}", e);
                return;
            }
        }
    }

    // Get the current executable
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            arf_eprintln!("Error: Failed to get current executable: {}", e);
            return;
        }
    };

    // Get command-line arguments (skip the program name, we'll use current_exe instead)
    // Also filter out any existing --with-r-version argument if we're switching versions
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    if version.is_some() {
        // Remove existing --with-r-version arguments
        args = filter_r_version_args(args);
        // Add the new version
        args.push("--with-r-version".to_string());
        args.push(version.unwrap().to_string());
    }

    // Clear R-related environment variables so the new process can set them fresh.
    // This is important when switching R versions, as LD_LIBRARY_PATH may be set
    // for the old R version.
    // SAFETY: We're about to exec(), so there are no other threads that could race.
    if version.is_some() {
        unsafe {
            std::env::remove_var("R_HOME");
            std::env::remove_var("LD_LIBRARY_PATH");
        }
    }

    // Build the command
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let mut cmd = Command::new(&exe);
        cmd.args(&args);

        // exec() replaces the current process - this should not return
        let err = cmd.exec();
        arf_eprintln!("Error: Failed to restart: {}", err);
    }

    #[cfg(not(unix))]
    {
        // On non-Unix platforms, spawn a new process and exit
        // This is not as clean as exec(), but works
        match Command::new(&exe)
            .args(&args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(_) => {
                // Exit the current process
                std::process::exit(0);
            }
            Err(e) => {
                arf_eprintln!("Error: Failed to restart: {}", e);
            }
        }
    }
}

/// Filter out --with-r-version and its value from command-line arguments.
fn filter_r_version_args(args: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    let mut skip_next = false;

    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }

        if arg == "--with-r-version" {
            // Skip this and the next argument (the version value)
            skip_next = true;
            continue;
        }

        if arg.starts_with("--with-r-version=") {
            // Skip --with-r-version=value form
            continue;
        }

        result.push(arg);
    }

    result
}
