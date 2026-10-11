//! Top-level CLI option scope and R source origin validation.

use crate::app::explicit_r_source::{
    ExplicitRSource, ExplicitRSourceRequest, explicit_r_source_request,
};
use crate::cli::{Cli, Commands, ConfigAction, RCommand, RSourceArgs};
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

pub(crate) fn normalize_r_source_args(
    args: &mut RSourceArgs,
) -> Result<Option<ExplicitRSourceRequest>, crate::app::explicit_r_source::ExplicitRSourceError> {
    let has_cli_selection =
        args.r_home.is_some() || args.r_version.is_some() || args.r_provider.is_some();
    let (env_home, env_version, env_provider) = if !has_cli_selection {
        (
            std::env::var_os("ARF_R_HOME"),
            std::env::var_os("ARF_R_VERSION"),
            std::env::var_os("ARF_R_PROVIDER"),
        )
    } else {
        // A CLI request takes precedence over every R source environment value.
        // Do not even parse inherited values that cannot affect this invocation.
        (None, None, None)
    };
    let request = explicit_r_source_request(
        args.r_home.clone(),
        args.r_version.clone(),
        args.r_provider,
        env_home,
        env_version,
        env_provider,
    )?;
    if let Some(request) = &request {
        match &request.source {
            ExplicitRSource::Home(path) => args.r_home = Some(path.clone()),
            ExplicitRSource::Version(version) => args.r_version = Some(version.clone()),
            ExplicitRSource::Provider(provider) => args.r_provider = Some(*provider),
        }
    }
    Ok(request)
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
