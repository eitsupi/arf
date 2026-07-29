use clap::Args;

#[derive(Args, Debug)]
pub(crate) struct RHomeArgs {
    #[command(flatten)]
    pub(crate) r_source: super::shared::RSourceArgs,

    /// Print resolution details as JSON
    #[arg(long)]
    pub(crate) json: bool,
}
