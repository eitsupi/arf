//! Session information display.

use crate::config::RSourceStatus;
use crate::editor::prompt::get_r_version;
use crate::external::{formatter, rig};

use super::arf_println;
use super::state::PromptRuntimeConfig;

/// Display session information for the :info command.
pub fn display_session_info(
    prompt_config: &PromptRuntimeConfig,
    config_path: &Option<std::path::PathBuf>,
    r_history_path: &Option<std::path::PathBuf>,
    shell_history_path: &Option<std::path::PathBuf>,
    r_source_status: &RSourceStatus,
) {
    arf_println!("Session Information");
    println!("#");

    // arf version
    println!("#   arf version:    {}", env!("CARGO_PKG_VERSION"));

    // OS information
    println!(
        "#   OS:             {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH
    );

    // Config file path
    if let Some(path) = config_path {
        if path.exists() {
            println!("#   Config file:    {}", path.display());
        } else {
            println!(
                "#   Config file:    {} (not found, using defaults)",
                path.display()
            );
        }
    } else {
        println!("#   Config file:    (using defaults)");
    }

    // R version
    let r_version = get_r_version();
    if r_version.is_empty() {
        println!("#   R version:      (not available)");
    } else {
        println!("#   R version:      {}", r_version);
    }

    // R_HOME
    if let Ok(r_home) = std::env::var("R_HOME") {
        println!("#   R_HOME:         {}", r_home);
    }

    // R source (how R was resolved at startup)
    println!("#   R source:       {}", r_source_status.display());

    println!("#");

    // rig status
    if rig::rig_available() {
        print!("#   rig:            installed");
        if let Ok(versions) = rig::list_versions()
            && !versions.is_empty()
        {
            let version_list: Vec<_> = versions
                .iter()
                .map(|v| {
                    if v.default {
                        format!("{}*", v.name)
                    } else {
                        v.name.clone()
                    }
                })
                .collect();
            print!(" ({})", version_list.join(", "));
        }
        println!();
    } else {
        println!("#   rig:            not installed");
    }

    // Air (formatter) status
    if formatter::is_formatter_available() {
        println!("#   air:            installed");
    } else {
        println!("#   air:            not installed");
    }

    println!("#");

    // Current mode
    let mode = if prompt_config.is_shell_enabled() {
        "Shell"
    } else if prompt_config.is_reprex_enabled() {
        "R (reprex)"
    } else {
        "R"
    };
    println!("#   Current mode:   {}", mode);

    // Autoformat status (only relevant in reprex mode)
    if prompt_config.is_reprex_enabled() {
        let autoformat = if prompt_config.is_autoformat_enabled() {
            "enabled"
        } else {
            "disabled"
        };
        println!("#   Auto-format:    {}", autoformat);
    }

    // Current working directory
    if let Ok(cwd) = std::env::current_dir() {
        println!("#   Working dir:    {}", cwd.display());
    }

    println!("#");

    // History paths
    if let Some(path) = r_history_path {
        println!("#   R history:      {}", path.display());
    }
    if let Some(path) = shell_history_path {
        println!("#   Shell history:  {}", path.display());
    }

    println!("#");

    // R-related environment variables
    let env_vars = [
        "R_LIBS",
        "R_LIBS_USER",
        "R_LIBS_SITE",
        "R_PROFILE",
        "R_ENVIRON",
    ];
    let mut has_env = false;
    for var in &env_vars {
        if let Ok(value) = std::env::var(var) {
            if !has_env {
                println!("#   Environment:");
                has_env = true;
            }
            println!("#     {}={}", var, value);
        }
    }
}
