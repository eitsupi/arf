use crate::external::rig;
use clap::builder::PossibleValuesParser;
use clap::{Args, CommandFactory};
use clap_complete::{Shell, generate};
use std::io;

use super::Cli;

#[derive(Args, Debug)]
pub(crate) struct CompletionsArgs {
    /// The shell to generate completions for
    #[arg(value_enum)]
    pub(crate) shell: Shell,
}

impl Cli {
    /// Print shell completions to stdout.
    ///
    /// If rig is available, this will include completion values for `--with-r-version`
    /// based on installed R versions.
    ///
    /// TODO: Migrate to dynamic completions using clap_complete's CompleteEnv
    /// when it stabilizes, so completions are generated at TAB-press time
    /// rather than requiring regeneration after installing new R versions.
    pub fn print_completions(shell: Shell) {
        let mut cmd = Cli::command();

        // Inject R version completions from rig if available
        if let Some(possible_values) = Self::get_r_version_completions() {
            // Leak memory for 'static lifetime - acceptable since completions run once and exit
            let leaked: &'static [String] = Box::leak(possible_values.into_boxed_slice());
            let refs: Vec<&'static str> = leaked.iter().map(|s| s.as_str()).collect();
            cmd = cmd.mut_arg("r_version", |arg| {
                arg.value_parser(PossibleValuesParser::new(refs))
            });
        }

        generate(shell, &mut cmd, "arf", &mut io::stdout());
    }

    /// Get possible R version values from rig for shell completion.
    ///
    /// Returns None if rig is unavailable or has no versions installed.
    fn get_r_version_completions() -> Option<Vec<String>> {
        if !rig::rig_available() {
            return None;
        }

        let versions = rig::list_versions().ok()?;
        if versions.is_empty() {
            return None;
        }

        let mut values = vec!["default".to_string()];

        for v in &versions {
            // Add version name (e.g., "4.5.2")
            values.push(v.name.clone());
            // Add aliases (e.g., "release", "oldrel")
            for alias in &v.aliases {
                if !values.contains(alias) {
                    values.push(alias.clone());
                }
            }
        }

        Some(values)
    }

    /// Generate shell completions as a string for testing.
    #[cfg(test)]
    pub(super) fn generate_completions_string(shell: Shell) -> String {
        let mut cmd = Cli::command();
        let mut buf = Vec::new();
        generate(shell, &mut cmd, "arf", &mut buf);
        String::from_utf8(buf).expect("Completions should be valid UTF-8")
    }

    /// Generate help output for a subcommand path for testing.
    #[cfg(test)]
    pub(super) fn generate_help_string(subcommand_path: &[&str]) -> String {
        let mut cmd = Cli::command();
        for &name in subcommand_path {
            cmd = cmd
                .find_subcommand(name)
                .expect("Subcommand not found")
                .clone();
        }
        cmd.render_long_help().to_string()
    }
}
