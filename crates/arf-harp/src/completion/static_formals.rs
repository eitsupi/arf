//! Static function-formal completion for installed R packages.
//!
//! This provider deliberately covers only the unambiguous `pkg::name(` form
//! and its partial-argument variants.
//! It reads declared namespace exports and the installed code database without
//! evaluating R. Dynamic exports, export patterns, and runtime namespace state
//! remain the responsibility of R's completion oracle.

use crate::lib_paths::{cached_lib_paths, installed_package_dir};
use rd_rds::package::{
    DefaultPresence, FormalsInspection, InstalledCodeError, MetadataField, NamespaceImport,
    NamespaceMetadata, StoredKind,
};
use std::collections::HashSet;
use std::time::{Duration, Instant};

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
/// Resolution metadata is retained for every outcome so shadow-mode reports
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
/// completion insertion text. The R oracle remains responsible for formatting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticFormal {
    pub name: String,
    pub default: DefaultPresence,
}

/// An observation used by tests and migration experiments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticFormalsObservation {
    pub result: StaticFormalsResult,
    pub candidate_count: usize,
    pub elapsed: Duration,
    /// Static lookup intentionally performs no R evaluation.
    pub r_evaluations: usize,
}

/// Side-by-side result for migration experiments. This helper is intentionally
/// not used by [`crate::completion::get_completions`]: the R result remains the
/// user-facing result until a later, separately authorized migration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StaticFormalsShadow {
    pub static_observation: StaticFormalsObservation,
    pub r_candidates: Vec<String>,
    pub r_elapsed: Duration,
    /// Number of calls to the harp R-completion wrapper. This is not an
    /// instrumented count of embedded R evaluations in the wider console
    /// request (which may also call token and function helpers).
    pub r_oracle_calls: usize,
    /// An exact embedded-R evaluation count is not currently instrumented.
    pub r_evaluation_count: Option<usize>,
}

/// Return the instrumentation count for the completion spike.
#[cfg(feature = "completion-spike")]
pub fn r_evaluation_count() -> usize {
    super::r_ffi::r_evaluation_count()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StaticFormalsRequest {
    package: String,
    exported_name: String,
    partial: String,
    used_named: Vec<String>,
}

/// Return static formals for a completion request, if it has the supported
/// `pkg::foo(` shape. Unsupported input is represented by `None` so callers
/// can retain their existing completion path unchanged.
pub fn lookup(line: &str, cursor_pos: usize) -> Option<StaticFormalsResult> {
    let request = parse_request(line, cursor_pos)?;
    Some(lookup_request(&request))
}

/// Perform a lookup and record bounded migration observations.
pub fn observe(line: &str, cursor_pos: usize) -> Option<StaticFormalsObservation> {
    let request = parse_request(line, cursor_pos)?;
    let started = Instant::now();
    let result = lookup_request(&request);
    let candidate_count = match &result {
        StaticFormalsResult {
            outcome: StaticFormalsOutcome::Available(formals),
            ..
        } => formals.len(),
        StaticFormalsResult { .. } => 0,
    };
    Some(StaticFormalsObservation {
        result,
        candidate_count,
        elapsed: started.elapsed(),
        r_evaluations: 0,
    })
}

/// Build conservative insertion candidates from an available static result.
///
/// This helper is deliberately separate from the production completion path.
/// It returns formal names only, filters names already supplied by the user,
/// and preserves the request's partial prefix. Formatting of candidates for
/// the editor remains an R-oracle concern until a later migration.
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

/// Compare static metadata with the existing R completion oracle.
pub fn shadow(
    line: &str,
    cursor_pos: usize,
    timeout_ms: u64,
) -> crate::error::HarpResult<Option<StaticFormalsShadow>> {
    let Some(static_observation) = observe(line, cursor_pos) else {
        return Ok(None);
    };
    #[cfg(feature = "completion-spike")]
    let r_count_before = super::r_ffi::r_evaluation_count();
    let started = Instant::now();
    let r_candidates = super::get_completions(line, cursor_pos, timeout_ms)?;
    #[cfg(feature = "completion-spike")]
    let r_evaluation_count =
        Some(super::r_ffi::r_evaluation_count().saturating_sub(r_count_before));
    #[cfg(not(feature = "completion-spike"))]
    let r_evaluation_count = None;
    Ok(Some(StaticFormalsShadow {
        static_observation,
        r_candidates,
        r_elapsed: started.elapsed(),
        r_oracle_calls: 1,
        r_evaluation_count,
    }))
}

fn parse_request(line: &str, cursor_pos: usize) -> Option<StaticFormalsRequest> {
    let before_cursor = line.get(..cursor_pos.min(line.len()))?;
    let operator = before_cursor.rfind("::")?;
    // `rfind("::")` also finds the final pair in `:::`. Reject both triple
    // colon forms and a colon immediately before the selected operator.
    if before_cursor[..operator].ends_with(':') {
        return None;
    }

    let package_end = operator;
    let package_start = before_cursor[..package_end]
        .char_indices()
        .rev()
        .find_map(|(index, character)| {
            (!is_identifier_character(character)).then_some(index + character.len_utf8())
        })
        .unwrap_or(0);
    // The spike intentionally accepts only a top-level qualified call. This
    // avoids guessing through a path, nested call, or other expression.
    if !before_cursor[..package_start].trim().is_empty() {
        return None;
    }
    let package = &before_cursor[package_start..package_end];
    if !is_valid_identifier(package) {
        return None;
    }

    let function_start = operator + 2;
    let function_end = before_cursor[function_start..]
        .find('(')
        .map(|offset| function_start + offset)?;
    let function = &before_cursor[function_start..function_end];
    if !is_valid_identifier(function) {
        return None;
    }

    let arguments = &before_cursor[function_end + 1..];
    if arguments
        .chars()
        .any(|character| matches!(character, '(' | ')' | '"' | '\'' | '#'))
    {
        return None;
    }
    let (completed_arguments, current_argument) = arguments
        .rsplit_once(',')
        .map_or(("", arguments), |(completed, current)| (completed, current));
    let partial = current_argument.trim();
    if !partial.is_empty() && !is_valid_identifier(partial) {
        return None;
    }
    let used_named = completed_arguments
        .split(',')
        .filter_map(named_argument_name)
        .collect();

    Some(StaticFormalsRequest {
        package: package.to_owned(),
        exported_name: function.to_owned(),
        partial: partial.to_owned(),
        used_named,
    })
}

fn is_identifier_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '.' | '_')
}

fn is_valid_identifier(identifier: &str) -> bool {
    let mut characters = identifier.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    !first.is_ascii_digit() && characters.all(is_identifier_character)
}

/// Return a named argument's name when the segment contains a top-level,
/// standalone `=` separator. Comparison operators are expressions, not named
/// arguments, and must not affect the set of already-used formals.
fn named_argument_name(argument: &str) -> Option<String> {
    let mut depth = 0usize;
    for (index, character) in argument.char_indices() {
        match character {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '=' if depth == 0 => {
                let previous = argument[..index].chars().next_back();
                let next = argument[index + character.len_utf8()..].chars().next();
                if previous.is_some_and(|value| matches!(value, '=' | '!' | '<' | '>'))
                    || next.is_some_and(|value| matches!(value, '=' | '<' | '>'))
                {
                    continue;
                }
                let name = argument[..index].trim();
                if is_valid_identifier(name) {
                    return Some(name.to_owned());
                }
            }
            _ => {}
        }
    }
    None
}

fn lookup_request(request: &StaticFormalsRequest) -> StaticFormalsResult {
    let paths = cached_lib_paths();
    if paths.is_empty() {
        return result(
            request,
            None,
            None,
            StaticFormalsOutcome::Unresolved(
                "library paths have not been populated; static lookup is not authoritative"
                    .to_owned(),
            ),
        );
    }

    let Some(package_dir) = installed_package_dir(&paths, &request.package) else {
        return result(
            request,
            None,
            None,
            StaticFormalsOutcome::Unresolved(format!(
                "package {:?} is not installed in the cached library paths",
                request.package
            )),
        );
    };

    let metadata_path = package_dir.join("Meta/nsInfo.rds");
    let metadata_object = match rd_rds::file::read(&metadata_path) {
        Ok(object) => object,
        Err(error) => {
            return result(
                request,
                None,
                None,
                StaticFormalsOutcome::Unavailable(format!(
                    "failed to read namespace metadata: {error}"
                )),
            );
        }
    };
    let metadata = match NamespaceMetadata::from_object(&metadata_object) {
        Ok(metadata) => metadata,
        Err(error) => {
            return result(
                request,
                None,
                None,
                StaticFormalsOutcome::Unresolved(format!(
                    "failed to parse namespace metadata: {error}"
                )),
            );
        }
    };

    let source_name = match metadata.declared_exports() {
        MetadataField::Present(exports) => {
            let matches = exports
                .iter()
                .filter(|export| export.exported_name() == request.exported_name)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return result(
                    request,
                    None,
                    None,
                    StaticFormalsOutcome::Unresolved(format!(
                        "declared export {:?} has {} unambiguous source bindings",
                        request.exported_name,
                        matches.len()
                    )),
                );
            }
            matches[0].source_name().to_owned()
        }
        MetadataField::Missing => {
            return result(
                request,
                None,
                None,
                StaticFormalsOutcome::Unresolved(
                    "namespace metadata has no declared export field".to_owned(),
                ),
            );
        }
        MetadataField::Invalid(error) => {
            return result(
                request,
                None,
                None,
                StaticFormalsOutcome::Unresolved(format!("declared exports are invalid: {error}")),
            );
        }
        MetadataField::UnsupportedSchema { description } => {
            return result(
                request,
                None,
                None,
                StaticFormalsOutcome::Unresolved(format!(
                    "declared export schema is unsupported: {description}"
                )),
            );
        }
        _ => {
            return result(
                request,
                None,
                None,
                StaticFormalsOutcome::Unresolved(
                    "declared export metadata has an unknown state".to_owned(),
                ),
            );
        }
    };

    if let Some(reason) = imported_export_reason(&metadata, &source_name) {
        return result(
            request,
            Some(source_name),
            None,
            StaticFormalsOutcome::Unresolved(reason),
        );
    }

    let database = match rd_rds::package::InstalledCodeDb::open(&package_dir) {
        Ok(database) => database,
        Err(error) => {
            return result(
                request,
                Some(source_name.clone()),
                None,
                StaticFormalsOutcome::Unavailable(format!(
                    "failed to open installed code database: {error}"
                )),
            );
        }
    };
    let inspection = match database.inspect_stored_binding(&source_name) {
        Ok(inspection) => inspection,
        Err(error) => {
            return result(
                request,
                Some(source_name.clone()),
                None,
                inspection_error_outcome(&source_name, error),
            );
        }
    };
    let kind = inspection.kind();

    match inspection.formals() {
        FormalsInspection::Available(formals) => result(
            request,
            Some(source_name),
            Some(kind),
            StaticFormalsOutcome::Available(
                formals
                    .iter()
                    .map(|formal| StaticFormal {
                        name: formal.name().to_owned(),
                        default: formal.default(),
                    })
                    .collect(),
            ),
        ),
        FormalsInspection::NotApplicable(reason) => result(
            request,
            Some(source_name),
            Some(kind),
            StaticFormalsOutcome::NotApplicable(format!("formals are not applicable: {reason:?}")),
        ),
        FormalsInspection::Unavailable(reason) => result(
            request,
            Some(source_name),
            Some(kind),
            StaticFormalsOutcome::Unavailable(format!("formals are unavailable: {reason:?}")),
        ),
        _ => result(
            request,
            Some(source_name),
            Some(kind),
            StaticFormalsOutcome::Unavailable("formals inspection has an unknown state".to_owned()),
        ),
    }
}

fn result(
    request: &StaticFormalsRequest,
    source_name: Option<String>,
    kind: Option<StoredKind>,
    outcome: StaticFormalsOutcome,
) -> StaticFormalsResult {
    StaticFormalsResult {
        package: request.package.clone(),
        exported_name: request.exported_name.clone(),
        source_name,
        kind,
        partial: request.partial.clone(),
        used_named: request.used_named.clone(),
        outcome,
    }
}

fn imported_export_reason(metadata: &NamespaceMetadata, source_name: &str) -> Option<String> {
    let imports = match metadata.imports() {
        MetadataField::Present(imports) => imports,
        MetadataField::Missing => return None,
        MetadataField::Invalid(_) | MetadataField::UnsupportedSchema { .. } => {
            return Some("import metadata is invalid or unsupported".to_owned());
        }
        _ => return Some("import metadata has an unknown state".to_owned()),
    };
    for import in imports {
        match import {
            // An `import`/`importFrom` all declaration does not by itself
            // shadow an explicit export declaration. Export-pattern and
            // re-export cases without an explicit local declaration are
            // unresolved earlier because they have no matching export.
            NamespaceImport::All { .. } => {}
            NamespaceImport::From { names, .. }
                if names.iter().any(|name| name.local_name() == source_name) =>
            {
                return Some("export resolves through an imported/re-exported binding".to_owned());
            }
            _ => {}
        }
    }
    None
}

fn inspection_error_outcome(source_name: &str, error: InstalledCodeError) -> StaticFormalsOutcome {
    let unresolved = matches!(
        &error,
        InstalledCodeError::UnknownStoredBinding { .. }
            | InstalledCodeError::AmbiguousStoredBinding { .. }
    );
    let reason = format!("failed to inspect stored binding {source_name:?}: {error}");
    if unresolved {
        StaticFormalsOutcome::Unresolved(reason)
    } else {
        StaticFormalsOutcome::Unavailable(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        StaticFormal, StaticFormalsOutcome, StaticFormalsResult, candidates,
        inspection_error_outcome, named_argument_name, parse_request,
    };
    use rd_rds::package::DefaultPresence;
    use rd_rds::package::InstalledCodeError;

    #[test]
    fn parses_only_initial_supported_shape() {
        assert_eq!(
            parse_request("stats::lm(", 10),
            Some(super::StaticFormalsRequest {
                package: "stats".to_owned(),
                exported_name: "lm".to_owned(),
                partial: "".to_owned(),
                used_named: Vec::new(),
            })
        );
        assert_eq!(
            parse_request("stats::lm(fo", "stats::lm(fo".len()),
            Some(super::StaticFormalsRequest {
                package: "stats".to_owned(),
                exported_name: "lm".to_owned(),
                partial: "fo".to_owned(),
                used_named: Vec::new(),
            })
        );
        assert!(parse_request("stats:::lm(", "stats:::lm(".len()).is_none());
        assert!(parse_request("stats::lm(foo(", "stats::lm(foo(".len()).is_none());
        assert_eq!(
            parse_request("stats::lm(foo = 1, ", "stats::lm(foo = 1, ".len()),
            Some(super::StaticFormalsRequest {
                package: "stats".to_owned(),
                exported_name: "lm".to_owned(),
                partial: "".to_owned(),
                used_named: vec!["foo".to_owned()],
            })
        );
        assert_eq!(
            parse_request("stats::lm(foo = 1, fo", "stats::lm(foo = 1, fo".len()),
            Some(super::StaticFormalsRequest {
                package: "stats".to_owned(),
                exported_name: "lm".to_owned(),
                partial: "fo".to_owned(),
                used_named: vec!["foo".to_owned()],
            })
        );
        assert_eq!(
            parse_request("stats::lm(foo = 1, bar", "stats::lm(foo = 1, bar".len())
                .expect("partial argument should parse")
                .used_named,
            vec!["foo".to_owned()]
        );
        let comparison = parse_request("stats::lm(x == 1, ", "stats::lm(x == 1, ".len())
            .expect("comparison expression should retain static completion");
        assert!(comparison.used_named.is_empty());
        let comparison_result = StaticFormalsResult {
            package: "stats".to_owned(),
            exported_name: "lm".to_owned(),
            source_name: Some("lm".to_owned()),
            kind: None,
            partial: comparison.partial,
            used_named: comparison.used_named,
            outcome: StaticFormalsOutcome::Available(vec![StaticFormal {
                name: "x".to_owned(),
                default: DefaultPresence::Absent,
            }]),
        };
        assert_eq!(candidates(&comparison_result), Some(vec!["x".to_owned()]));
        assert!(parse_request("stats::lm(foo = 1", "stats::lm(foo = 1".len()).is_none());
        assert!(parse_request("stats::lm(foo +", "stats::lm(foo +".len()).is_none());
        assert!(parse_request("stats::lm", "stats::lm".len()).is_none());
        assert!(parse_request("../stats::lm(", "../stats::lm(".len()).is_none());
        assert!(parse_request("stats::lm(foo = f(1), ", "stats::lm(foo = f(1), ".len()).is_none());
        assert!(
            parse_request("stats::lm(foo = \"x\", ", "stats::lm(foo = \"x\", ".len()).is_none()
        );
        assert!(
            parse_request(
                "stats::lm(foo = 1 # comment",
                "stats::lm(foo = 1 # comment".len()
            )
            .is_none()
        );
    }

    #[test]
    fn candidates_filter_partial_used_names_and_duplicates() {
        let result = StaticFormalsResult {
            package: "stats".to_owned(),
            exported_name: "lm".to_owned(),
            source_name: Some("lm".to_owned()),
            kind: None,
            partial: "fo".to_owned(),
            used_named: vec!["formula".to_owned()],
            outcome: StaticFormalsOutcome::Available(vec![
                StaticFormal {
                    name: "formula".to_owned(),
                    default: DefaultPresence::Absent,
                },
                StaticFormal {
                    name: "foo".to_owned(),
                    default: DefaultPresence::Absent,
                },
                StaticFormal {
                    name: "foo".to_owned(),
                    default: DefaultPresence::Absent,
                },
            ]),
        };
        assert_eq!(candidates(&result), Some(vec!["foo".to_owned()]));
        assert_eq!(
            candidates(&StaticFormalsResult {
                outcome: StaticFormalsOutcome::Unavailable("unsafe".to_owned()),
                ..result
            }),
            None
        );
    }

    #[test]
    fn unsafe_binding_resolution_errors_fall_back_to_unresolved() {
        for error in [
            InstalledCodeError::UnknownStoredBinding {
                name: "missing".to_owned(),
            },
            InstalledCodeError::AmbiguousStoredBinding {
                name: "duplicate".to_owned(),
                count: 2,
            },
        ] {
            assert!(matches!(
                inspection_error_outcome("name", error),
                StaticFormalsOutcome::Unresolved(_)
            ));
        }
    }

    #[test]
    fn other_binding_errors_are_unavailable() {
        let error = InstalledCodeError::InvalidPackageDirectory {
            path: std::path::PathBuf::from("pkg"),
        };
        assert!(matches!(
            inspection_error_outcome("name", error),
            StaticFormalsOutcome::Unavailable(_)
        ));
    }

    #[test]
    fn comparisons_are_not_named_argument_separators() {
        for argument in ["x == 1", "x != 1", "x <= 1", "x >= 1"] {
            assert_eq!(named_argument_name(argument), None, "{argument}");
        }
        assert_eq!(named_argument_name("foo = 1"), Some("foo".to_owned()));
        assert_eq!(named_argument_name("foo=1"), Some("foo".to_owned()));
    }
}
