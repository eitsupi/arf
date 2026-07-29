use crate::external::rig;
use clap::builder::PossibleValuesParser;
use clap::{Args, Command, CommandFactory};
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
            cmd = Self::with_r_version_possible_values(cmd, possible_values);
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

    /// Apply possible R version values to every command that accepts an R version.
    fn with_r_version_possible_values(mut cmd: Command, possible_values: Vec<String>) -> Command {
        // Leak memory for 'static lifetime - acceptable since completions run once and exit
        let leaked: &'static [String] = Box::leak(possible_values.into_boxed_slice());
        let refs: Vec<&'static str> = leaked.iter().map(|s| s.as_str()).collect();
        cmd = cmd.mut_arg("r_version", |arg| {
            arg.value_parser(PossibleValuesParser::new(refs.iter().copied()))
        });
        for subcommand in ["headless", "r-home"] {
            cmd = cmd.mut_subcommand(subcommand, |subcommand| {
                subcommand.mut_arg("r_version", |arg| {
                    arg.value_parser(PossibleValuesParser::new(refs.iter().copied()))
                })
            });
        }
        cmd
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r_version_possible_values_are_applied_to_all_commands() {
        let possible_values = vec!["default".to_string(), "4.5.0".to_string()];
        let cmd = Cli::with_r_version_possible_values(Cli::command(), possible_values);

        let expected = vec!["default".to_string(), "4.5.0".to_string()];
        let r_version_values = |command: &Command| {
            command
                .get_arguments()
                .find(|arg| arg.get_id() == "r_version")
                .expect("r_version argument should exist")
                .get_possible_values()
                .iter()
                .map(|value| value.get_name().to_string())
                .collect::<Vec<_>>()
        };

        assert_eq!(r_version_values(&cmd), expected);
        for subcommand_name in ["headless", "r-home"] {
            let subcommand = cmd
                .get_subcommands()
                .find(|candidate| candidate.get_name() == subcommand_name)
                .expect("subcommand should exist");
            assert_eq!(
                r_version_values(subcommand),
                expected,
                "{subcommand_name} r_version should have possible values"
            );
        }

        let eval = cmd
            .get_arguments()
            .find(|arg| arg.get_id() == "eval")
            .expect("eval argument should exist");
        assert!(
            eval.get_possible_values().is_empty(),
            "eval should not have possible values"
        );
    }
}
