//! Catalog of meta commands (`:help`, `:cd`, ...) and their argument choices.
//!
//! The REPL dispatcher and the completion engine both read this table, so a
//! command or subcommand is declared in exactly one place.

/// Identity of a meta command, shared by all of its spellings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetaCommand {
    Help,
    Info,
    Shell,
    R,
    System,
    Reprex,
    Commands,
    Restart,
    Switch,
    Changelog,
    History,
    Cd,
    Pushd,
    Popd,
    Ipc,
    Quit,
}

/// Definition of one spelling of a meta command.
pub(crate) struct MetaCommandDef {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
    /// Whether this command takes an argument (e.g., `:switch 4.4`).
    /// If true, completion appends a trailing space.
    pub(crate) takes_argument: bool,
    pub(crate) command: MetaCommand,
}

const fn def(
    name: &'static str,
    description: &'static str,
    takes_argument: bool,
    command: MetaCommand,
) -> MetaCommandDef {
    MetaCommandDef {
        name,
        description,
        takes_argument,
        command,
    }
}

/// Available meta commands, including aliases.
pub(crate) const META_COMMANDS: &[MetaCommandDef] = &[
    def("help", "Search R help", false, MetaCommand::Help),
    def("h", "Search R help (alias)", false, MetaCommand::Help),
    def("info", "Show session information", false, MetaCommand::Info),
    def(
        "session",
        "Show session information (alias)",
        false,
        MetaCommand::Info,
    ),
    def("shell", "Enter shell mode", false, MetaCommand::Shell),
    def("r", "Return to R mode", false, MetaCommand::R),
    def(
        "system",
        "Execute system command",
        true,
        MetaCommand::System,
    ),
    def(
        "reprex",
        "Set reprex mode (on, off, or format)",
        true,
        MetaCommand::Reprex,
    ),
    def(
        "commands",
        "Show available commands",
        false,
        MetaCommand::Commands,
    ),
    def(
        "cmds",
        "Show available commands (alias)",
        false,
        MetaCommand::Commands,
    ),
    def("restart", "Restart R session", false, MetaCommand::Restart),
    def(
        "restart!",
        "Restart R session (skip confirmation)",
        false,
        MetaCommand::Restart,
    ),
    def(
        "switch",
        "Restart with different R version (requires rig)",
        true,
        MetaCommand::Switch,
    ),
    def(
        "switch!",
        "Restart with different R version, skip confirmation (requires rig)",
        true,
        MetaCommand::Switch,
    ),
    def(
        "changelog",
        "Show arf changelog",
        false,
        MetaCommand::Changelog,
    ),
    def(
        "history",
        "Manage command history",
        true,
        MetaCommand::History,
    ),
    def("cd", "Change working directory", true, MetaCommand::Cd),
    def(
        "pushd",
        "Push directory and change to it",
        true,
        MetaCommand::Pushd,
    ),
    def("popd", "Pop directory from stack", false, MetaCommand::Popd),
    def(
        "ipc",
        "IPC server and send-policy management",
        true,
        MetaCommand::Ipc,
    ),
    def("quit", "Quit arf", false, MetaCommand::Quit),
    def("exit", "Quit arf", false, MetaCommand::Quit),
];

/// Find the definition for a command word typed after `:`.
///
/// Matching is exact, except that `R` is accepted as an alias of `r`.
pub(crate) fn lookup_meta_command(name: &str) -> Option<&'static MetaCommandDef> {
    let name = if name == "R" { "r" } else { name };
    META_COMMANDS.iter().find(|def| def.name == name)
}

/// One accepted value of a meta command argument.
pub(crate) struct ArgumentChoice {
    pub(crate) name: &'static str,
    pub(crate) description: &'static str,
}

const fn choice(name: &'static str, description: &'static str) -> ArgumentChoice {
    ArgumentChoice { name, description }
}

/// Values accepted by `:reprex`.
pub(crate) const REPREX_MODES: &[ArgumentChoice] = &[
    choice("on", "Enable reprex output"),
    choice("off", "Disable reprex output"),
    choice("format", "Format code before reprex evaluation"),
];

/// Subcommands of `:history`.
pub(crate) const HISTORY_SUBCOMMANDS: &[ArgumentChoice] = &[
    choice("browse", "Browse and manage command history"),
    choice("clear", "Clear command history"),
    choice("schema", "Display database schema and R examples"),
];

/// Targets of `:history clear`.
pub(crate) const HISTORY_CLEAR_TARGETS: &[ArgumentChoice] = &[
    choice("r", "Clear R mode history"),
    choice("shell", "Clear shell mode history"),
    choice("all", "Clear all history"),
];

/// Targets of `:history browse`.
pub(crate) const HISTORY_BROWSE_TARGETS: &[ArgumentChoice] = &[
    choice("r", "Browse R mode history"),
    choice("shell", "Browse shell mode history"),
];

/// Subcommands of `:ipc`.
pub(crate) const IPC_SUBCOMMANDS: &[ArgumentChoice] = &[
    choice("start", "Start the IPC server"),
    choice("stop", "Stop the IPC server"),
    choice("status", "Show IPC server status"),
    choice(
        "send-policy",
        "Change interactive send approval for this process",
    ),
];

/// Values accepted by `:ipc send-policy`.
pub(crate) const IPC_SEND_POLICIES: &[ArgumentChoice] = &[
    choice("prompt", "Require approval for every interactive send"),
    choice("allow", "Allow sends for the current REPL process"),
];

/// Join the names of a choice list with `separator`.
pub(crate) fn join_choice_names(choices: &[ArgumentChoice], separator: &str) -> String {
    choices
        .iter()
        .map(|choice| choice.name)
        .collect::<Vec<_>>()
        .join(separator)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_names_are_unique() {
        for (index, def) in META_COMMANDS.iter().enumerate() {
            assert!(
                META_COMMANDS[index + 1..]
                    .iter()
                    .all(|other| other.name != def.name),
                "duplicate meta command name: {}",
                def.name
            );
        }
    }

    #[test]
    fn every_listed_name_resolves_to_its_own_definition() {
        for def in META_COMMANDS {
            let found = lookup_meta_command(def.name).expect("listed command resolves");
            assert_eq!(found.name, def.name);
            assert_eq!(found.command, def.command);
        }
    }

    #[test]
    fn lookup_accepts_uppercase_r_and_rejects_unknown_names() {
        assert_eq!(
            lookup_meta_command("R").map(|def| def.command),
            Some(MetaCommand::R)
        );
        assert!(lookup_meta_command("history!").is_none());
        assert!(lookup_meta_command("").is_none());
        assert!(lookup_meta_command("nonexistent").is_none());
    }

    #[test]
    fn joined_choice_names_keep_declaration_order() {
        assert_eq!(join_choice_names(REPREX_MODES, "|"), "on|off|format");
        assert_eq!(join_choice_names(IPC_SEND_POLICIES, "|"), "prompt|allow");
        assert_eq!(
            join_choice_names(IPC_SUBCOMMANDS, ", "),
            "start, stop, status, send-policy"
        );
    }
}
