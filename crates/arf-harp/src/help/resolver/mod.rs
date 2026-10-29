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
    Page {
        page: PreparedHelpPage,
        title: String,
    },
    /// Several metadata rows match. The caller must request an explicit choice
    /// and prepare only the selected candidate with
    /// [`HelpTargetResolver::prepare_candidate`].
    Candidates(Vec<HelpTopic>),
    NotFound,
}

#[derive(Clone, Copy)]
enum CurrentPackage<'a> {
    Exact(&'a PreparedHelpPage),
    Named(&'a str),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResolveMode {
    Viewer,
    Ipc,
}

/// Resolver using a library-path snapshot obtained before entering the viewer.
///
/// All operations use installed files, never R evaluation. Package discovery
/// and Rd metadata are loaded lazily as needed for lookup and page titles.
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

    /// Resolve an IPC topic using the same flow as pager cross-reference lookup.
    /// A `help_key` selects the exact compiled-help page without aliases.
    pub fn resolve_ipc_target(
        &mut self,
        topic: &str,
        package: Option<&str>,
        current_package: Option<&str>,
        help_key: Option<&str>,
    ) -> HarpResult<HelpResolution> {
        self.resolve_flow(
            topic,
            package,
            current_package.map(CurrentPackage::Named),
            help_key,
            ResolveMode::Ipc,
        )
    }

    /// Resolve a pager link in the exact installed copy that supplied its page.
    pub fn resolve(
        &mut self,
        current: &PreparedHelpPage,
        target: &HelpTarget,
    ) -> HarpResult<HelpResolution> {
        self.resolve_flow(
            &target.topic,
            target.package.as_deref(),
            Some(CurrentPackage::Exact(current)),
            None,
            ResolveMode::Viewer,
        )
    }

    fn resolve_flow(
        &mut self,
        topic: &str,
        package: Option<&str>,
        current: Option<CurrentPackage<'_>>,
        help_key: Option<&str>,
        mode: ResolveMode,
    ) -> HarpResult<HelpResolution> {
        if let Some(package) = package {
            let Some(dir) = installed_package_dir(&self.library_paths, package) else {
                return Ok(HelpResolution::NotFound);
            };
            return self.resolve_direct(package, &dir, topic, help_key, mode);
        }

        let (current_name, current_dir) = match current {
            Some(CurrentPackage::Exact(page)) => {
                (Some(page.package.clone()), Some(page.package_dir.clone()))
            }
            Some(CurrentPackage::Named(name)) => (
                Some(name.to_owned()),
                installed_package_dir(&self.library_paths, name),
            ),
            None => (None, None),
        };
        if let (Some(name), Some(dir)) = (&current_name, &current_dir) {
            let resolved = self.resolve_direct(name, dir, topic, help_key, mode)?;
            if !matches!(&resolved, HelpResolution::NotFound) {
                return Ok(resolved);
            }
        }

        let mut candidates = Vec::new();
        let packages = self
            .packages
            .get_or_insert_with(|| installed_package_dirs(&self.library_paths))
            .clone();
        for (name, dir) in &packages {
            if current_name.as_deref() == Some(name) {
                continue;
            }
            candidates.extend(self.matching_topics(name, dir, topic, mode)?);
        }
        self.finish_candidates(candidates, mode)
    }

    fn resolve_direct(
        &mut self,
        package: &str,
        dir: &Path,
        topic: &str,
        help_key: Option<&str>,
        mode: ResolveMode,
    ) -> HarpResult<HelpResolution> {
        let page = if let Some(help_key) = help_key {
            let db = open_package_help_db(dir, package, topic, help_key)?;
            Some(prepare_page(&db, dir, package, topic, help_key)?)
        } else {
            lookup_in_package(dir, package, topic)?
        };
        Ok(match page {
            Some(page) => HelpResolution::Page {
                title: if mode == ResolveMode::Ipc {
                    self.title_for_key(dir, package, &page.help_key)
                } else {
                    String::new()
                },
                page,
            },
            None => HelpResolution::NotFound,
        })
    }

    fn package_index(
        &mut self,
        package: &str,
        dir: &Path,
        topic: &str,
    ) -> HarpResult<Option<&HelpTopicIndex>> {
        if !self.indexes.contains_key(dir) {
            let index = HelpTopicIndex::read_installed(dir)
                .map_err(|source| database_error(package, topic, topic, source))?;
            self.indexes.insert(dir.to_owned(), index);
        }
        Ok(self.indexes.get(dir).and_then(Option::as_ref))
    }

    fn title_for_key(&mut self, dir: &Path, package: &str, help_key: &str) -> String {
        self.package_index(package, dir, help_key)
            .ok()
            .flatten()
            .and_then(|index| {
                index
                    .entries()
                    .find(|entry| entry.topic_key() == Some(help_key))
                    .map(|entry| entry.title.as_str().unwrap_or_default().to_owned())
            })
            .unwrap_or_default()
    }

    fn matching_topics(
        &mut self,
        package: &str,
        dir: &Path,
        topic: &str,
        mode: ResolveMode,
    ) -> HarpResult<Vec<HelpTopic>> {
        match self.package_index(package, dir, topic) {
            Ok(Some(index)) => Ok(matching_entries(
                index,
                dir,
                package,
                topic,
                mode == ResolveMode::Ipc,
            )),
            Ok(None) => Ok(Vec::new()),
            Err(error) if mode == ResolveMode::Ipc => {
                log::warn!(
                    "Skipping unreadable help index for package {package:?} at {}: {error}",
                    dir.display()
                );
                Ok(Vec::new())
            }
            Err(error) => Err(error),
        }
    }

    fn finish_candidates(
        &self,
        mut candidates: Vec<HelpTopic>,
        mode: ResolveMode,
    ) -> HarpResult<HelpResolution> {
        let ipc = mode == ResolveMode::Ipc;
        let mut seen = HashSet::new();
        candidates.retain(|entry| match &entry.help_key {
            Some(key) => seen.insert((entry.package_dir.clone(), Some(key.clone()))),
            None if ipc => true,
            None => seen.insert((entry.package_dir.clone(), None)),
        });
        if ipc {
            candidates.sort_by(|left, right| {
                (&left.package, &left.topic, &left.help_key).cmp(&(
                    &right.package,
                    &right.topic,
                    &right.help_key,
                ))
            });
        }
        match candidates.len() {
            0 => Ok(HelpResolution::NotFound),
            1 => {
                let candidate = &candidates[0];
                let page = Self::prepare_candidate(candidate)?;
                Ok(HelpResolution::Page {
                    page,
                    title: if ipc {
                        candidate.title.clone()
                    } else {
                        String::new()
                    },
                })
            }
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
    metadata_topic: bool,
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
            let aliases = entry.aliases.iter().flatten().cloned().collect::<Vec<_>>();
            let display_topic = if metadata_topic {
                entry
                    .name
                    .as_str()
                    .filter(|name| aliases.iter().any(|alias| alias == name))
                    .or_else(|| entry.aliases.first().and_then(Option::as_deref))
                    .unwrap_or(topic)
            } else {
                topic
            };
            Some(HelpTopic {
                package: package.to_owned(),
                package_dir: dir.to_owned(),
                topic: display_topic.to_owned(),
                aliases,
                help_key: key.map(str::to_owned),
                title: entry.title.as_str().unwrap_or_default().to_owned(),
                entry_type: "help".to_owned(),
            })
        })
        .collect()
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
