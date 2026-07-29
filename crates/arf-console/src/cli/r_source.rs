/// Resolve the headless R source as a pair of mutually exclusive options.
///
/// If either subcommand option is set, the subcommand pair determines the
/// source. Otherwise, the top-level pair is used.
pub(crate) fn resolve_headless_r_source<'a>(
    top_level: (Option<&'a std::path::PathBuf>, Option<&'a String>),
    subcommand: (Option<&'a std::path::PathBuf>, Option<&'a String>),
) -> (Option<&'a std::path::PathBuf>, Option<&'a String>) {
    if subcommand.0.is_some() || subcommand.1.is_some() {
        subcommand
    } else {
        top_level
    }
}
