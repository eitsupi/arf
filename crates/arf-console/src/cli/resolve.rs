use clap::{Args, Subcommand};

/// R source commands.
#[derive(Args, Debug)]
pub(crate) struct RArgs {
    #[command(subcommand)]
    pub(crate) command: RCommand,
}

#[derive(Subcommand, Debug)]
pub(crate) enum RCommand {
    /// Resolve the R installation arf would use without starting R.
    Resolve(ResolveArgs),
}

#[derive(Args, Debug)]
pub(crate) struct ResolveArgs {
    #[command(flatten)]
    pub(crate) r_source: super::shared::RSourceArgs,
}
