/// Builder for R initialization arguments, shared between REPL and headless modes.
///
/// Headless mode always uses `--no-save --no-restore-data` (save/restore are
/// only meaningful for interactive sessions), while REPL mode supports
/// `--save` and `--restore`.
pub struct RArgsBuilder<'a> {
    pub vanilla: bool,
    pub no_environ: bool,
    pub no_site_file: bool,
    pub no_init_file: bool,
    /// Use `--save` instead of `--no-save`. Only relevant for REPL mode.
    pub save: bool,
    /// Use `--restore` instead of `--no-restore-data`. Only relevant for REPL mode.
    pub restore: bool,
    pub max_connections: Option<u32>,
    pub max_ppsize: Option<u32>,
    pub min_nsize: Option<&'a str>,
    pub min_vsize: Option<&'a str>,
}

impl RArgsBuilder<'_> {
    /// Build the R initialization arguments vector.
    pub fn build(&self) -> Vec<String> {
        let mut args = Vec::new();

        // Always add --quiet (we handle our own banner)
        args.push("--quiet".to_string());

        // --vanilla combines: --no-environ --no-site-file --no-init-file --no-save --no-restore
        if self.vanilla {
            args.push("--no-environ".to_string());
            args.push("--no-site-file".to_string());
            args.push("--no-init-file".to_string());
            args.push("--no-save".to_string());
            args.push("--no-restore-data".to_string());
        } else {
            if self.no_environ {
                args.push("--no-environ".to_string());
            }
            if self.no_site_file {
                args.push("--no-site-file".to_string());
            }
            if self.no_init_file {
                args.push("--no-init-file".to_string());
            }

            // Save/restore flags
            // Default behavior is --no-save --no-restore (like radian)
            if self.save {
                args.push("--save".to_string());
            } else {
                args.push("--no-save".to_string());
            }

            if self.restore {
                args.push("--restore".to_string());
            } else {
                args.push("--no-restore-data".to_string());
            }
        }

        // Memory tuning flags - forward to R
        if let Some(n) = self.max_connections {
            args.push(format!("--max-connections={n}"));
        }
        if let Some(n) = self.max_ppsize {
            args.push(format!("--max-ppsize={n}"));
        }
        if let Some(n) = self.min_nsize {
            args.push(format!("--min-nsize={n}"));
        }
        if let Some(n) = self.min_vsize {
            args.push(format!("--min-vsize={n}"));
        }

        // Always interactive (Unix only - Windows uses Rstart.r_interactive)
        #[cfg(unix)]
        args.push("--interactive".to_string());

        args
    }
}

impl super::Cli {
    /// Returns the script file path from `-f`/`--file`.
    pub fn script_file(&self) -> Option<&std::path::PathBuf> {
        self.file.as_ref()
    }

    /// Generate R initialization arguments based on CLI flags.
    ///
    /// Returns a vector of R arguments like ["--quiet", "--no-save", "--no-restore"].
    pub fn r_args(&self) -> Vec<String> {
        RArgsBuilder {
            vanilla: self.r_compat.vanilla,
            no_environ: self.r_compat.no_environ,
            no_site_file: self.r_compat.no_site_file,
            no_init_file: self.r_compat.no_init_file,
            save: self.save,
            restore: self.restore_data || self.restore,
            max_connections: self.r_compat.max_connections,
            max_ppsize: self.r_compat.max_ppsize,
            min_nsize: self.r_compat.min_nsize.as_deref(),
            min_vsize: self.r_compat.min_vsize.as_deref(),
        }
        .build()
    }
}
