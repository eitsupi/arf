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

    /// Resolve an IPC topic using the pager's direct package lookup before
    /// falling back to metadata search across installed packages.
    ///
    /// A `help_key` selects the exact compiled-help page and can be used without an
    /// aliases file. The IPC boundary requires it to be paired with a package.
    pub fn resolve_ipc_target(
        &mut self,
        topic: &str,
        package: Option<&str>,
        current_package: Option<&str>,
        help_key: Option<&str>,
    ) -> HarpResult<HelpResolution> {
        let packages = self
            .packages
            .get_or_insert_with(|| installed_package_dirs(&self.library_paths))
            .clone();

        if let Some(package) = package {
            let Some((name, dir)) = packages.iter().find(|(name, _)| name == package) else {
                return Ok(HelpResolution::NotFound);
            };
            if let Some(help_key) = help_key {
                return self.page_by_key(name, dir, topic, help_key);
            }
            return self.direct_page(name, dir, topic);
        }

        if let Some(current_package) = current_package
            && let Some((name, dir)) = packages.iter().find(|(name, _)| name == current_package)
        {
            let result = self.direct_page(name, dir, topic);
            if !matches!(&result, Ok(HelpResolution::NotFound)) {
                return result;
            }
        }

        let mut matches = Vec::new();
        for (name, dir) in &packages {
            if Some(name.as_str()) == current_package {
                continue;
            }
            matches.extend(self.matching_topics_or_warn(name, dir, topic));
        }
        Self::finish_ipc_resolution(matches)
    }

    fn direct_page(
        &mut self,
        package: &str,
        dir: &Path,
        topic: &str,
    ) -> HarpResult<HelpResolution> {
        match lookup_in_package(dir, package, topic)? {
            Some(page) => {
                let title = self.title_for_key(dir, package, &page.help_key);
                Ok(HelpResolution::Page { page, title })
            }
            None => Ok(HelpResolution::NotFound),
        }
    }

    fn page_by_key(
        &mut self,
        package: &str,
        dir: &Path,
        topic: &str,
        help_key: &str,
    ) -> HarpResult<HelpResolution> {
        let candidate = HelpTopic {
            package: package.to_owned(),
            package_dir: dir.to_owned(),
            topic: topic.to_owned(),
            aliases: Vec::new(),
            help_key: Some(help_key.to_owned()),
            title: self.title_for_key(dir, package, help_key),
            entry_type: "help".to_owned(),
        };
        let page = Self::prepare_candidate(&candidate)?;
        Ok(HelpResolution::Page {
            page,
            title: candidate.title,
        })
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

    fn matching_topics_or_warn(
        &mut self,
        package: &str,
        dir: &Path,
        topic: &str,
    ) -> Vec<HelpTopic> {
        match self.package_index(package, dir, topic).map(|index| {
            index.map_or_else(Vec::new, |index| {
                matching_entries(index, dir, package, topic, CandidateDisplay::Metadata)
            })
        }) {
            Ok(topics) => topics,
            Err(error) => {
                log::warn!(
                    "Skipping unreadable help index for package {package:?} at {}: {error}",
                    dir.display()
                );
                Vec::new()
            }
        }
    }

    fn finish_ipc_resolution(mut candidates: Vec<HelpTopic>) -> HarpResult<HelpResolution> {
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
            0 => Ok(HelpResolution::NotFound),
            1 => {
                let candidate = &candidates[0];
                let page = Self::prepare_candidate(candidate)?;
                Ok(HelpResolution::Page {
                    page,
                    title: candidate.title.clone(),
                })
            }
            _ => Ok(HelpResolution::Candidates(candidates)),
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
            return self.direct_page(package, &dir, &target.topic);
        }

        let direct = self.direct_page(&current.package, &current.package_dir, &target.topic)?;
        if !matches!(&direct, HelpResolution::NotFound) {
            return Ok(direct);
        }

        let packages = self
            .packages
            .get_or_insert_with(|| installed_package_dirs(&self.library_paths))
            .clone();
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();
        for (package, dir) in packages.iter().filter(|(name, _)| name != &current.package) {
            let Some(index) = self.package_index(package, dir, &target.topic)? else {
                continue;
            };
            for candidate in matching_entries(
                index,
                dir,
                package,
                &target.topic,
                CandidateDisplay::Requested,
            ) {
                if seen.insert((dir.clone(), candidate.help_key.clone())) {
                    candidates.push(candidate);
                }
            }
        }
        match candidates.as_slice() {
            [] => Ok(HelpResolution::NotFound),
            [candidate] => {
                let page = Self::prepare_candidate(candidate)?;
                Ok(HelpResolution::Page {
                    page,
                    title: candidate.title.clone(),
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
    display: CandidateDisplay,
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
            Some(project_index_entry(
                entry, dir, package, topic, key, display,
            ))
        })
        .collect()
}

/// Convert one selected metadata row to a help candidate while preserving the
/// caller's display-topic policy.
fn project_index_entry(
    entry: &rd_helpdb::HelpTopicEntry,
    dir: &Path,
    package: &str,
    fallback_topic: &str,
    help_key: Option<&str>,
    display: CandidateDisplay,
) -> HelpTopic {
    let aliases = entry.aliases.iter().flatten().cloned().collect::<Vec<_>>();
    let display_topic = match display {
        CandidateDisplay::Requested => fallback_topic,
        CandidateDisplay::Metadata => entry
            .name
            .as_str()
            .filter(|name| aliases.iter().any(|alias| alias == name))
            .or_else(|| entry.aliases.first().and_then(Option::as_deref))
            .unwrap_or(fallback_topic),
    };
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

#[derive(Clone, Copy)]
enum CandidateDisplay {
    Requested,
    Metadata,
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
