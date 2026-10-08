//! Top-level CLI option scope and R source origin validation.

use crate::app::resolve::RSourceOrigin;
use crate::cli::{Cli, Commands, ConfigAction, RCommand};
use crate::config::ConfigFileSource;
use clap::parser::ValueSource;
use clap::{ArgMatches, Command};
use std::path::Path;

pub(crate) fn effective_config_path(cli: &Cli) -> Option<&Path> {
    match &cli.command {
        None => cli.r_source.config.as_deref(),
        Some(Commands::History(_)) => cli.r_source.config.as_deref(),
        Some(Commands::Headless(args)) => args.r_source.config.as_deref(),
        Some(Commands::R(args)) => match &args.command {
            RCommand::Resolve(args) => args.r_source.config.as_deref(),
        },
        Some(Commands::Config(args)) => match &args.action {
            ConfigAction::Check { config } => config.as_deref(),
            ConfigAction::Init { .. } => None,
        },
        Some(Commands::Completions(_) | Commands::Ipc(_)) => None,
    }
}

pub(crate) fn config_file_source(matches: &ArgMatches) -> ConfigFileSource {
    match matches.value_source("config") {
        Some(ValueSource::CommandLine) => ConfigFileSource::CommandLine,
        Some(ValueSource::EnvVariable) => ConfigFileSource::Environment,
        _ => ConfigFileSource::Default,
    }
}

pub(crate) fn r_source_origin(matches: &ArgMatches) -> Option<RSourceOrigin> {
    let resolve_matches = matches
        .subcommand_matches("r")
        .and_then(|matches| matches.subcommand_matches("resolve"))?;

    if resolve_matches.value_source("r_home") == Some(ValueSource::CommandLine)
        || resolve_matches.value_source("r_version") == Some(ValueSource::CommandLine)
    {
        Some(RSourceOrigin::Cli)
    } else if resolve_matches.value_source("r_home") == Some(ValueSource::EnvVariable)
        || resolve_matches.value_source("r_version") == Some(ValueSource::EnvVariable)
    {
        Some(RSourceOrigin::Environment)
    } else {
        None
    }
}

pub(crate) fn validate_top_level_scope(command: &Command, matches: &ArgMatches) {
    if matches.subcommand_name().is_none() {
        return;
    }

    let mut path = Vec::new();
    let mut path_commands = Vec::new();
    let mut current_command = command;
    let mut current_matches = matches;

    while let Some((subcommand_name, nested_matches)) = current_matches.subcommand() {
        let subcommand = current_command
            .find_subcommand(subcommand_name)
            .expect("parsed subcommand must exist");
        path.push(subcommand_name.to_owned());
        path_commands.push(subcommand);
        current_command = subcommand;
        current_matches = nested_matches;
    }

    let subcommand_path = path.join(" ");
    let final_subcommand = path_commands
        .last()
        .map(|subcommand| (*subcommand).clone())
        .expect("parsed subcommand path must not be empty");

    for arg in command.get_arguments() {
        let Some(long) = arg.get_long() else {
            continue;
        };

        // These checks have deliberately custom errors below and must retain
        // their existing wording and ordering.
        if matches!(arg.get_id().as_str(), "eval" | "file")
            || is_history_option_allowed(&path, long)
        {
            continue;
        }

        if matches.value_source(arg.get_id().as_str()) != Some(ValueSource::CommandLine) {
            continue;
        }

        let mut subcommand_command = final_subcommand.clone();
        subcommand_command.set_bin_name(format!("arf {subcommand_path}"));

        if let Some(subcommand_arg) = path_commands.iter().rev().find_map(|subcommand| {
            subcommand
                .get_arguments()
                .find(|subcommand_arg| subcommand_arg.get_long() == Some(long))
        }) {
            let value_names = if matches!(
                subcommand_arg.get_action(),
                clap::ArgAction::SetTrue | clap::ArgAction::SetFalse
            ) {
                String::new()
            } else {
                subcommand_arg
                    .get_value_names()
                    .map(|names| {
                        names
                            .iter()
                            .map(|name| format!("<{}>", name.as_str()))
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default()
            };
            let corrected_form = if value_names.is_empty() {
                format!("arf {subcommand_path} --{long}")
            } else {
                format!("arf {subcommand_path} --{long} {value_names}")
            };

            subcommand_command
                .error(
                    clap::error::ErrorKind::ArgumentConflict,
                    format!(
                        "'--{long}' was given before the '{subcommand_path}' subcommand, where it has no effect\n\n  tip: place it after the subcommand instead:\n       {corrected_form}"
                    ),
                )
                .exit();
        }

        let console_form = format!("arf --{long}");
        subcommand_command
            .error(
                clap::error::ErrorKind::ArgumentConflict,
                format!(
                    "'--{long}' is not used by the '{subcommand_path}' subcommand\n\n  tip: it applies to the interactive console, which takes no subcommand:\n       {console_form}"
                ),
            )
            .exit();
    }
}

fn is_history_option_allowed(path: &[String], long: &str) -> bool {
    // History subcommands opt in only to options they consume; `--no-history` remains session-only.
    matches!(long, "config" | "history-dir")
        && path.len() == 2
        && path[0] == "history"
        && matches!(path[1].as_str(), "import" | "export" | "schema")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::CommandFactory;

    #[test]
    fn config_file_source_uses_clap_value_source() {
        let mut env = crate::test_utils::lock_env();
        env.unset("ARF_CONFIG");
        env.unset("ARF_R_HOME");
        env.unset("ARF_R_VERSION");

        let default_matches = Cli::command().try_get_matches_from(["arf"]).unwrap();
        assert_eq!(
            config_file_source(&default_matches),
            ConfigFileSource::Default
        );

        env.set("ARF_CONFIG", "/same/config.toml");
        let environment_matches = Cli::command().try_get_matches_from(["arf"]).unwrap();
        assert_eq!(
            config_file_source(&environment_matches),
            ConfigFileSource::Environment
        );

        let cli_matches = Cli::command()
            .try_get_matches_from(["arf", "--config", "/same/config.toml"])
            .unwrap();
        assert_eq!(
            config_file_source(&cli_matches),
            ConfigFileSource::CommandLine
        );
    }
}
