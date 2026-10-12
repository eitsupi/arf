//! Static resolution of a request against installed package metadata.

use super::{StaticFormal, StaticFormalsOutcome, StaticFormalsRequest, StaticFormalsResult};
use crate::lib_paths::{cached_lib_paths, installed_package_dir};
use rd_rds::package::{
    FormalsInspection, InstalledCodeError, MetadataField, NamespaceImport, NamespaceMetadata,
    StoredKind,
};

pub(super) fn lookup_request(request: &StaticFormalsRequest) -> StaticFormalsResult {
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

pub(super) fn inspection_error_outcome(
    source_name: &str,
    error: InstalledCodeError,
) -> StaticFormalsOutcome {
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
