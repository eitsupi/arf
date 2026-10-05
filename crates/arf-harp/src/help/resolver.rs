//! Static cross-reference resolution for one help viewer session.

use super::{HelpTarget, HelpTopic, open_package_help_db, package_help_markdown_from_db};
use crate::error::{HarpError, HarpResult};
use crate::help_bridge::PreparedHelpPage;
use crate::lib_paths::{installed_package_dir, installed_package_dirs};
use rd_helpdb::{HelpTopicIndex, PackageHelpDb};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Result of resolving a help cross-reference without evaluating R.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelpResolution {
    Page(PreparedHelpPage),
    /// Several metadata rows match. The caller must request an explicit choice
    /// and prepare only the selected candidate with
    /// [`HelpTargetResolver::prepare_candidate`].
    Candidates(Vec<HelpTopic>),
    NotFound,
}

/// Resolver using a library-path snapshot obtained before entering the viewer.
///
/// All operations use installed files, never R evaluation. Package discovery
/// and Rd metadata are loaded lazily after an unqualified current-package miss.
pub struct HelpTargetResolver {
    library_paths: Vec<String>,
    packages: Option<Vec<(String, PathBuf)>>,
    indexes: HashMap<PathBuf, Option<HelpTopicIndex>>,
}

impl HelpTargetResolver {
    pub fn new(library_paths: Vec<String>) -> Self {
        Self {
            library_paths,
            packages: None,
            indexes: HashMap::new(),
        }
    }

    /// Resolve relative to the exact installed copy that supplied `current`.
    ///
    /// Qualified targets select the first installed copy in library order.
    /// Unqualified targets prefer the current copy, then search other package
    /// metadata. Another copy of the current package is never substituted.
    /// Corrupt or unreadable databases/metadata are errors, not missing topics.
    pub fn resolve(
        &mut self,
        current: &PreparedHelpPage,
        target: &HelpTarget,
    ) -> HarpResult<HelpResolution> {
        if let Some(package) = &target.package {
            let Some(dir) = installed_package_dir(&self.library_paths, package) else {
                return Ok(HelpResolution::NotFound);
            };
            return lookup_in_package(&dir, package, &target.topic)
                .map(|page| page.map_or(HelpResolution::NotFound, HelpResolution::Page));
        }

        if let Some(page) =
            lookup_in_package(&current.package_dir, &current.package, &target.topic)?
        {
            return Ok(HelpResolution::Page(page));
        }

        let packages = self
            .packages
            .get_or_insert_with(|| installed_package_dirs(&self.library_paths));
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for (package, dir) in packages.iter().filter(|(name, _)| name != &current.package) {
            if !self.indexes.contains_key(dir) {
                let index = HelpTopicIndex::read_installed(dir).map_err(|source| {
                    database_error(package, &target.topic, &target.topic, source)
                })?;
                self.indexes.insert(dir.clone(), index);
            }
            let Some(index) = self.indexes[dir].as_ref() else {
                continue;
            };
            for entry in index.entries() {
                let key = entry.topic_key().filter(|key| !key.is_empty());
                if !entry
                    .aliases
                    .iter()
                    .flatten()
                    .any(|alias| alias == &target.topic)
                    && key != Some(target.topic.as_str())
                {
                    continue;
                }
                if !seen.insert((dir.clone(), key.map(str::to_owned))) {
                    continue;
                }
                candidates.push(HelpTopic {
                    package: package.clone(),
                    package_dir: dir.clone(),
                    topic: target.topic.clone(),
                    aliases: entry.aliases.iter().flatten().cloned().collect(),
                    help_key: key.map(str::to_owned),
                    title: entry.title.as_str().unwrap_or_default().to_owned(),
                    entry_type: "help".to_owned(),
                });
            }
        }
        match candidates.as_slice() {
            [] => Ok(HelpResolution::NotFound),
            [candidate] => Self::prepare_candidate(candidate).map(HelpResolution::Page),
            _ => Ok(HelpResolution::Candidates(candidates)),
        }
    }

    /// Prepare a selected resolver candidate using its original package copy
    /// and known database key. A missing metadata key falls back to alias/key
    /// lookup within that same copy.
    pub fn prepare_candidate(candidate: &HelpTopic) -> HarpResult<PreparedHelpPage> {
        if let Some(key) = &candidate.help_key {
            let db = open_package_help_db(
                &candidate.package_dir,
                &candidate.package,
                &candidate.topic,
                key,
            )?;
            return prepare_page(
                &db,
                &candidate.package_dir,
                &candidate.package,
                &candidate.topic,
                key,
            );
        }
        lookup_in_package(&candidate.package_dir, &candidate.package, &candidate.topic)?.ok_or_else(
            || {
                database_error(
                    &candidate.package,
                    &candidate.topic,
                    &candidate.topic,
                    rd_helpdb::Error::UnknownTopic {
                        topic: candidate.topic.clone(),
                    },
                )
            },
        )
    }
}

fn lookup_in_package(
    dir: &Path,
    package: &str,
    topic: &str,
) -> HarpResult<Option<PreparedHelpPage>> {
    let db = open_package_help_db(dir, package, topic, topic)?;
    let alias = db
        .resolve_alias(topic)
        .map_err(|source| database_error(package, topic, topic, source))?;
    let key = if let Some(key) = alias {
        key
    } else if db.topics().any(|key| key == topic) {
        topic
    } else {
        return Ok(None);
    };
    prepare_page(&db, dir, package, topic, key).map(Some)
}

fn prepare_page(
    db: &PackageHelpDb,
    dir: &Path,
    package: &str,
    topic: &str,
    key: &str,
) -> HarpResult<PreparedHelpPage> {
    Ok(PreparedHelpPage {
        package_dir: dir.to_owned(),
        package: package.to_owned(),
        display_topic: topic.to_owned(),
        help_key: key.to_owned(),
        markdown: package_help_markdown_from_db(db, topic, key, package)?,
    })
}

fn database_error(package: &str, topic: &str, key: &str, source: rd_helpdb::Error) -> HarpError {
    HarpError::HelpDatabase {
        package: package.to_owned(),
        topic: topic.to_owned(),
        key: key.to_owned(),
        source: Box::new(source),
    }
}

#[cfg(test)]
mod tests;
