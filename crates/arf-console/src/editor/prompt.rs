//! Prompt formatting with placeholder expansion.
//!
//! Supports the following placeholders in prompt format strings:
//! - `{version}` - R version (e.g., "4.4.0")
//! - `{cwd}` - Current working directory (full path)
//! - `{cwd_short}` - Current working directory (basename only)
//! - `{shell}` - Shell name from $SHELL (e.g., "bash", "zsh")
//! - `{vi}` - Vi mode indicator (insert/normal, or empty for Emacs mode)

use crate::config::prompt::ViSymbol;
use std::env;
use std::path::Path;

/// Vi editing mode for prompt display.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViMode {
    /// Insert mode (typing text).
    #[default]
    Insert,
    /// Normal mode (command mode).
    Normal,
}

/// Prompt formatter that expands placeholders.
#[derive(Debug, Clone)]
pub struct PromptFormatter {
    /// Cached R version string (e.g., "4.4.0").
    r_version: String,
    /// Cached shell name from $SHELL (e.g., "bash", "zsh").
    shell_name: String,
}

impl PromptFormatter {
    /// Create a new prompt formatter.
    ///
    /// Caches static values (R version, shell name) at creation time.
    pub fn new() -> Self {
        let r_version = get_r_version();
        let shell_name = get_shell_name();
        Self {
            r_version,
            shell_name,
        }
    }

    /// Expand placeholders in the format string.
    ///
    /// # Placeholders
    ///
    /// - `{version}` - R version (e.g., "4.4.0")
    /// - `{cwd}` - Current working directory (full path)
    /// - `{cwd_short}` - Current working directory (basename only)
    /// - `{shell}` - Shell name from $SHELL (e.g., "bash", "zsh")
    pub fn format(&self, template: &str) -> String {
        let mut result = template.to_string();

        // Static placeholders (cached)
        result = result.replace("{version}", &self.r_version);
        result = result.replace("{shell}", &self.shell_name);

        // Dynamic placeholders (computed each time)
        if result.contains("{cwd}") || result.contains("{cwd_short}") {
            let cwd = get_cwd();
            let cwd_short = get_cwd_short(&cwd);
            result = result.replace("{cwd}", &cwd);
            result = result.replace("{cwd_short}", &cwd_short);
        }

        result
    }

    /// Expand placeholders including vi mode indicator.
    ///
    /// # Placeholders
    ///
    /// - `{version}` - R version (e.g., "4.4.0")
    /// - `{cwd}` - Current working directory (full path)
    /// - `{cwd_short}` - Current working directory (basename only)
    /// - `{shell}` - Shell name from $SHELL (e.g., "bash", "zsh")
    /// - `{vi}` - Vi mode indicator (from symbol config, or empty if None)
    ///
    /// # Arguments
    ///
    /// - `template` - The format string with placeholders
    /// - `vi_mode` - Current vi mode (None for Emacs mode)
    /// - `vi_symbol` - Symbols to use for insert/normal mode
    #[allow(dead_code)]
    pub fn format_with_vi(
        &self,
        template: &str,
        vi_mode: Option<ViMode>,
        vi_symbol: &ViSymbol,
    ) -> String {
        let mut result = self.format(template);

        // Expand {vi} placeholder based on mode
        if result.contains("{vi}") {
            let vi_text = match vi_mode {
                Some(ViMode::Insert) => &vi_symbol.insert,
                Some(ViMode::Normal) => &vi_symbol.normal,
                None => &vi_symbol.non_vi, // Non-vi modes (Emacs, etc.)
            };
            result = result.replace("{vi}", vi_text);
        }

        result
    }
}

impl Default for PromptFormatter {
    fn default() -> Self {
        Self::new()
    }
}

/// Get R version string (e.g., "4.4.0").
pub fn get_r_version() -> String {
    if arf_libr::r_library().is_err() {
        return String::new();
    }

    // Evaluate paste0(R.version$major, ".", R.version$minor)
    // Use invisible() to suppress console output
    match arf_harp::eval_string(r#"invisible(paste0(R.version$major, ".", R.version$minor))"#) {
        Ok(result) => extract_string(result.sexp()).unwrap_or_default(),
        Err(_) => String::new(),
    }
}

/// Extract a string from an R SEXP.
fn extract_string(sexp: arf_libr::SEXP) -> Option<String> {
    let lib = arf_libr::r_library().ok()?;
    unsafe {
        if (lib.rf_isstring)(sexp) == 0 || (lib.rf_length)(sexp) == 0 {
            return None;
        }
        let elt = (lib.string_elt)(sexp, 0);
        let cstr = (lib.r_charsxp)(elt);
        if cstr.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr(cstr)
            .to_str()
            .ok()
            .map(|s| s.to_string())
    }
}

/// Get the current working directory.
fn get_cwd() -> String {
    env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".to_string())
}

/// Get the short form of the current working directory (basename).
fn get_cwd_short(cwd: &str) -> String {
    Path::new(cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(cwd)
        .to_string()
}

/// Get the shell name for shell mode prompt display.
///
/// On Unix: Extracts the basename from $SHELL (e.g., "/bin/bash" -> "bash").
/// On Windows: Returns "cmd" since shell mode uses cmd.exe.
fn get_shell_name() -> String {
    #[cfg(windows)]
    {
        // Shell mode on Windows uses cmd.exe (see execute_shell_command in repl.rs)
        "cmd".to_string()
    }
    #[cfg(not(windows))]
    {
        env::var("SHELL")
            .ok()
            .and_then(|shell_path| {
                Path::new(&shell_path)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "sh".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_placeholders() {
        let formatter = PromptFormatter {
            r_version: "4.4.0".to_string(),
            shell_name: "bash".to_string(),
        };

        // Default prompt should remain unchanged
        assert_eq!(formatter.format("r> "), "r> ");
        assert_eq!(formatter.format("+  "), "+  ");
    }

    #[test]
    fn test_version_placeholder() {
        let formatter = PromptFormatter {
            r_version: "4.4.0".to_string(),
            shell_name: "bash".to_string(),
        };

        assert_eq!(formatter.format("R {version}> "), "R 4.4.0> ");
        assert_eq!(formatter.format("[{version}] r> "), "[4.4.0] r> ");
    }

    #[test]
    fn test_cwd_placeholders() {
        let formatter = PromptFormatter {
            r_version: "4.4.0".to_string(),
            shell_name: "bash".to_string(),
        };

        // These will use actual cwd, just check they don't panic
        let result = formatter.format("{cwd}> ");
        assert!(result.ends_with("> "));

        let result = formatter.format("{cwd_short}> ");
        assert!(result.ends_with("> "));
    }

    #[test]
    fn test_shell_placeholder() {
        let formatter = PromptFormatter {
            r_version: "4.4.0".to_string(),
            shell_name: "zsh".to_string(),
        };

        assert_eq!(formatter.format("[{shell}] $ "), "[zsh] $ ");
        assert_eq!(formatter.format("{shell}> "), "zsh> ");
    }

    #[test]
    fn test_vi_placeholder_insert_mode() {
        let formatter = PromptFormatter {
            r_version: "4.4.0".to_string(),
            shell_name: "bash".to_string(),
        };
        let symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };

        let result = formatter.format_with_vi("{vi}r> ", Some(ViMode::Insert), &symbol);
        assert_eq!(result, "[I] r> ");
    }

    #[test]
    fn test_vi_placeholder_normal_mode() {
        let formatter = PromptFormatter {
            r_version: "4.4.0".to_string(),
            shell_name: "bash".to_string(),
        };
        let symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };

        let result = formatter.format_with_vi("{vi}r> ", Some(ViMode::Normal), &symbol);
        assert_eq!(result, "[N] r> ");
    }

    #[test]
    fn test_vi_placeholder_non_vi_mode() {
        let formatter = PromptFormatter {
            r_version: "4.4.0".to_string(),
            shell_name: "bash".to_string(),
        };
        let symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };

        // Non-vi mode (vi_mode = None) should use non_vi symbol
        let result = formatter.format_with_vi("{vi}r> ", None, &symbol);
        assert_eq!(result, "[E] r> ");
    }

    #[test]
    fn test_vi_placeholder_with_empty_symbols() {
        let formatter = PromptFormatter {
            r_version: "4.4.0".to_string(),
            shell_name: "bash".to_string(),
        };
        let symbol = ViSymbol::default(); // All empty

        // Even with vi mode, empty symbols should expand to empty string
        let result = formatter.format_with_vi("{vi}r> ", Some(ViMode::Insert), &symbol);
        assert_eq!(result, "r> ");
    }

    #[test]
    fn test_vi_placeholder_combined_with_others() {
        let formatter = PromptFormatter {
            r_version: "4.4.0".to_string(),
            shell_name: "bash".to_string(),
        };
        let symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };

        // Test combining {vi} with other placeholders
        let result = formatter.format_with_vi("{vi}R {version}> ", Some(ViMode::Normal), &symbol);
        assert_eq!(result, "[N] R 4.4.0> ");
    }

    #[test]
    fn test_no_vi_placeholder_format_with_vi_still_works() {
        let formatter = PromptFormatter {
            r_version: "4.4.0".to_string(),
            shell_name: "bash".to_string(),
        };
        let symbol = ViSymbol {
            insert: "[I] ".to_string(),
            normal: "[N] ".to_string(),
            non_vi: "[E] ".to_string(),
        };

        // Template without {vi} placeholder should work normally
        let result = formatter.format_with_vi("R {version}> ", Some(ViMode::Insert), &symbol);
        assert_eq!(result, "R 4.4.0> ");
    }
}
