//! Static cross-reference resolution for one help viewer session.

use super::markdown::{open_package_help_db, package_help_markdown_from_db};
use super::{HelpTarget, HelpTopic};
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

/// IPC resolution result that keeps the selected metadata title alongside the
/// prepared page, without changing pager resolution behavior.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcHelpResolution {
    Page {
        page: PreparedHelpPage,
        title: String,
    },
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

    /// Resolve an IPC topic, reading the current package before other indexes.
    ///
    /// This deliberately remains separate from pager resolution: IPC returns
    /// ambiguity candidates and carries the title from the selected metadata
    /// row, while the viewer keeps its established behavior. A `help_key` is
    /// an exact metadata-row selector and is only accepted with an explicit
    /// package by the IPC boundary.
    pub fn resolve_ipc_target(
        &mut self,
        topic: &str,
        package: Option<&str>,
        current_package: Option<&str>,
        help_key: Option<&str>,
    ) -> HarpResult<IpcHelpResolution> {
        let packages = self
            .packages
            .get_or_insert_with(|| installed_package_dirs(&self.library_paths))
            .clone();

        if let Some(package) = package {
            let Some((name, dir)) = packages.iter().find(|(name, _)| name == package) else {
                return Ok(IpcHelpResolution::NotFound);
            };
            let matches = self.ipc_matches_in_package(name, dir, topic, help_key)?;
            return Self::finish_ipc_resolution(matches);
        }

        if let Some(current_package) = current_package
            && let Some((name, dir)) = packages.iter().find(|(name, _)| name == current_package)
        {
            let matches = self.ipc_matches_in_package_or_warn(name, dir, topic, help_key);
            if !matches.is_empty() {
                return Self::finish_ipc_resolution(matches);
            }
        }

        let mut matches = Vec::new();
        for (name, dir) in &packages {
            if Some(name.as_str()) == current_package {
                continue;
            }
            matches.extend(self.ipc_matches_in_package_or_warn(name, dir, topic, help_key));
        }
        Self::finish_ipc_resolution(matches)
    }

    fn ipc_matches_in_package(
        &mut self,
        package: &str,
        dir: &Path,
        topic: &str,
        help_key: Option<&str>,
    ) -> HarpResult<Vec<HelpTopic>> {
        if !self.indexes.contains_key(dir) {
            let index = HelpTopicIndex::read_installed(dir)
                .map_err(|source| database_error(package, topic, topic, source))?;
            self.indexes.insert(dir.to_owned(), index);
        }
        Ok(self.indexes[dir].as_ref().map_or_else(Vec::new, |index| {
            if let Some(help_key) = help_key {
                matching_key(index, dir, package, help_key)
            } else {
                matching_entries(index, dir, package, topic)
            }
        }))
    }

    fn ipc_matches_in_package_or_warn(
        &mut self,
        package: &str,
        dir: &Path,
        topic: &str,
        help_key: Option<&str>,
    ) -> Vec<HelpTopic> {
        match self.ipc_matches_in_package(package, dir, topic, help_key) {
            Ok(matches) => matches,
            Err(error) => {
                log::warn!(
                    "Skipping unreadable help index for package {package:?} at {}: {error}",
                    dir.display()
                );
                Vec::new()
            }
        }
    }

    fn finish_ipc_resolution(mut candidates: Vec<HelpTopic>) -> HarpResult<IpcHelpResolution> {
        let mut seen = HashSet::new();
        candidates.retain(|entry| {
            entry
                .help_key
                .as_ref()
                .is_none_or(|key| seen.insert((entry.package_dir.clone(), Some(key.clone()))))
        });
        candidates.sort_by(|left, right| {
            (&left.package, &left.topic, &left.help_key).cmp(&(
                &right.package,
                &right.topic,
                &right.help_key,
            ))
        });
        match candidates.len() {
            0 => Ok(IpcHelpResolution::NotFound),
            1 => {
                let candidate = &candidates[0];
                let page = Self::prepare_candidate(candidate)?;
                Ok(IpcHelpResolution::Page {
                    page,
                    title: candidate.title.clone(),
                })
            }
            _ => Ok(IpcHelpResolution::Candidates(candidates)),
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
            let matches_alias = |entry: &rd_helpdb::HelpTopicEntry| {
                entry
                    .aliases
                    .iter()
                    .flatten()
                    .any(|alias| alias == &target.topic)
            };
            // Match the direct lookup's alias-first policy within each package.
            // Multiple alias rows remain candidates for explicit selection.
            let has_alias = index.entries().any(matches_alias);
            for entry in index.entries() {
                let key = entry.topic_key().filter(|key| !key.is_empty());
                let matches = if has_alias {
                    matches_alias(entry)
                } else {
                    key == Some(target.topic.as_str())
                };
                if !matches {
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

/// Match a topic within one metadata index, preferring aliases over database
/// keys just as the pager resolver does.
fn matching_entries(
    index: &HelpTopicIndex,
    dir: &Path,
    package: &str,
    topic: &str,
) -> Vec<HelpTopic> {
    let matches_alias = |entry: &rd_helpdb::HelpTopicEntry| {
        entry.aliases.iter().flatten().any(|alias| alias == topic)
    };
    let has_alias = index.entries().any(matches_alias);
    index
        .entries()
        .filter_map(|entry| {
            let key = entry.topic_key().filter(|key| !key.is_empty());
            let matches = if has_alias {
                matches_alias(entry)
            } else {
                key == Some(topic)
            };
            if !matches {
                return None;
            }
            Some(project_ipc_entry(entry, dir, package, topic, key))
        })
        .collect()
}

/// Select metadata by its compiled-help key without applying alias-first
/// lookup. This lets an IPC client retrieve a previously returned candidate
/// even when its topic string is also an alias for another metadata row.
fn matching_key(
    index: &HelpTopicIndex,
    dir: &Path,
    package: &str,
    help_key: &str,
) -> Vec<HelpTopic> {
    index
        .entries()
        .filter(|entry| entry.topic_key() == Some(help_key))
        .map(|entry| project_ipc_entry(entry, dir, package, help_key, Some(help_key)))
        .collect()
}

fn project_ipc_entry(
    entry: &rd_helpdb::HelpTopicEntry,
    dir: &Path,
    package: &str,
    fallback_topic: &str,
    help_key: Option<&str>,
) -> HelpTopic {
    let aliases = entry.aliases.iter().flatten().cloned().collect::<Vec<_>>();
    let display_topic = entry
        .name
        .as_str()
        .filter(|name| aliases.iter().any(|alias| alias == name))
        .or_else(|| entry.aliases.first().and_then(Option::as_deref))
        .unwrap_or(fallback_topic);
    HelpTopic {
        package: package.to_owned(),
        package_dir: dir.to_owned(),
        topic: display_topic.to_owned(),
        aliases,
        help_key: help_key.map(str::to_owned),
        title: entry.title.as_str().unwrap_or_default().to_owned(),
        entry_type: "help".to_owned(),
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
