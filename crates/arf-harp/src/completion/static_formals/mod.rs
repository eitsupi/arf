//! Static function-formal completion for installed R packages.
//!
//! This provider deliberately covers only the unambiguous `pkg::name(` form
//! and its partial-argument variants.
//! It reads declared namespace exports and the installed code database without
//! evaluating R. Dynamic exports, export patterns, and runtime namespace state
//! remain the responsibility of R's completion oracle.

mod lookup;
mod parse;
#[cfg(test)]
mod tests;

use parse::{is_valid_identifier, parse_request};
use rd_rds::package::{DefaultPresence, StoredKind};
use std::collections::HashSet;

/// The outcome of inspecting one statically resolved binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaticFormalsOutcome {
    /// A closure's formal names were inspected successfully.
    Available(Vec<StaticFormal>),
    /// The binding was found, but formals do not apply to it.
    NotApplicable(String),
    /// The binding or its formals could not be inspected safely.
    Unavailable(String),
    /// Static metadata did not identify one source binding.
    Unresolved(String),
}

/// The result of a static formals lookup.
///
/// Resolution metadata is retained for every outcome so test observations
/// can distinguish export resolution failures from code inspection failures.
/// In particular, `NotApplicable` still records the stored kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticFormalsResult {
    pub package: String,
    pub exported_name: String,
    pub source_name: Option<String>,
    pub kind: Option<StoredKind>,
    pub partial: String,
    pub used_named: Vec<String>,
    pub outcome: StaticFormalsOutcome,
}

/// A formal name with the serialized default-presence bit kept separate from
/// the insertion text produced by [`production_candidates`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticFormal {
    pub name: String,
    pub default: DefaultPresence,
}

/// Runtime mode for installed-package static formal completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StaticFormalsMode {
    /// Use the existing R completion oracle only.
    #[default]
    Off,
    /// Use static formals when they are unambiguous, otherwise use R.
    PreferStatic,
}

/// Runtime policy for static formal completion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StaticFormalsPolicy {
    pub mode: StaticFormalsMode,
    pub excluded_packages: Vec<String>,
    pub excluded_functions: Vec<String>,
}

impl StaticFormalsPolicy {
    /// Return a policy that disables static inspection.
    pub fn off() -> Self {
        Self::default()
    }

    /// Check exclusions using the requested public package/function names.
    pub fn excludes(&self, package: &str, function: &str) -> bool {
        let qualified_name = format!("{package}::{function}");
        self.excluded_packages.iter().any(|name| name == package)
            || self
                .excluded_functions
                .iter()
                .any(|name| name == &qualified_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StaticFormalsRequest {
    package: String,
    exported_name: String,
    partial: String,
    used_named: Vec<String>,
}

/// Resolve a request unless its public package or function name is excluded.
/// Exclusions are checked after parsing but before any installed metadata or
/// code database access.
pub fn lookup_with_policy(
    line: &str,
    cursor_pos: usize,
    policy: &StaticFormalsPolicy,
) -> Option<StaticFormalsResult> {
    if policy.mode == StaticFormalsMode::Off {
        return None;
    }
    let request = parse_request(line, cursor_pos)?;
    if policy.excludes(&request.package, &request.exported_name) {
        return None;
    }
    Some(lookup::lookup_request(&request))
}

/// Build conservative insertion candidates from an available static result.
///
/// Returns formal names only, filters names already supplied by the user, and
/// preserves the request's partial prefix. [`production_candidates`] formats
/// these names for insertion in the editor.
pub fn candidates(result: &StaticFormalsResult) -> Option<Vec<String>> {
    let StaticFormalsResult {
        outcome: StaticFormalsOutcome::Available(formals),
        partial,
        used_named,
        ..
    } = result
    else {
        return None;
    };

    let mut seen = HashSet::new();
    Some(
        formals
            .iter()
            .filter(|formal| {
                (partial.is_empty() || formal.name.starts_with(partial))
                    && !used_named.iter().any(|name| name == &formal.name)
            })
            .filter(|formal| seen.insert(formal.name.clone()))
            .map(|formal| formal.name.clone())
            .collect(),
    )
}

/// Format static candidates for the experimental production path.
///
/// R 4.5.2 uses `name=` for ordinary formal candidates while `...` is already
/// a complete token and is returned unchanged. This helper intentionally does
/// not add global or other R-completer candidates.
pub fn production_candidates(result: &StaticFormalsResult) -> Option<Vec<String>> {
    if result.kind != Some(StoredKind::Closure) {
        return None;
    }
    let candidates = candidates(result)?;
    // Keep static-first conservative: a malformed or reserved formal in the
    // selected prefix makes the whole result unsafe to insert. A later R
    // fallback can apply backtick quoting or other language-specific rules.
    if candidates
        .iter()
        .any(|name| name != "..." && !is_valid_identifier(name))
    {
        return None;
    }
    let formatted = candidates
        .into_iter()
        .map(|name| {
            if name == "..." {
                name
            } else {
                format!("{name}=")
            }
        })
        .collect::<Vec<_>>();
    // An empty static result falls back to R so non-formal candidates remain available.
    (!formatted.is_empty()).then_some(formatted)
}
