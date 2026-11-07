//! Static cross-reference resolution for installed help pages.

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

#[derive(Clone, Copy)]
pub enum CurrentPackage<'a> {
    /// Search the exact copy that supplied this previously prepared page.
    Exact(&'a PreparedHelpPage),
    /// Search the first installed copy of this package name.
    Named(&'a str),
}

#[derive(Clone, Copy, Default)]
pub enum CandidateDisplay {
    /// Retain the requested topic as the candidate display name.
    #[default]
    Requested,
    /// Use the metadata row's representative alias as the display name.
    Metadata,
}

#[derive(Clone, Copy)]
pub struct ResolveOptions {
    /// Skip unreadable metadata indexes while searching all installed packages.
    pub skip_broken_indexes: bool,
    /// Choose how each candidate's display topic is projected from metadata.
    pub candidate_display: CandidateDisplay,
    /// Remove duplicate alias-only metadata rows within each package copy.
    pub deduplicate_keyless: bool,
}

impl Default for ResolveOptions {
    fn default() -> Self {
        Self {
            skip_broken_indexes: false,
            candidate_display: CandidateDisplay::Requested,
            deduplicate_keyless: true,
        }
    }
}

/// Resolver using a snapshot of the installed library paths.
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

    /// Resolve a link in the exact installed copy that supplied its page.
    pub fn resolve(
        &mut self,
        current: &PreparedHelpPage,
        target: &HelpTarget,
    ) -> HarpResult<HelpResolution> {
        self.resolve_target(
            &target.topic,
            target.package.as_deref(),
            Some(CurrentPackage::Exact(current)),
            None,
            ResolveOptions::default(),
        )
    }

    /// Resolve a topic using explicit, current-package, then global lookup.
    /// A `help_key` selects the exact compiled-help page without aliases during
    /// explicit or current-package lookup; global lookup matches the topic.
    pub fn resolve_target(
        &mut self,
        topic: &str,
        package: Option<&str>,
        current: Option<CurrentPackage<'_>>,
        help_key: Option<&str>,
        options: ResolveOptions,
    ) -> HarpResult<HelpResolution> {
        if let Some(package) = package {
            let Some(dir) = installed_package_dir(&self.library_paths, package) else {
                return Ok(HelpResolution::NotFound);
            };
            return self.resolve_direct(package, &dir, topic, help_key);
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
            let resolved = self.resolve_direct(name, dir, topic, help_key)?;
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
            candidates.extend(self.matching_topics(name, dir, topic, options)?);
        }
        self.finish_candidates(candidates, options)
    }

    fn resolve_direct(
        &mut self,
        package: &str,
        dir: &Path,
        topic: &str,
        help_key: Option<&str>,
    ) -> HarpResult<HelpResolution> {
        let page = if let Some(help_key) = help_key {
            let db = open_package_help_db(dir, package, topic, help_key)?;
            if !db.topics().any(|key| key == help_key) {
                return Ok(HelpResolution::NotFound);
            }
            Some(prepare_page(&db, dir, package, topic, help_key)?)
        } else {
            lookup_in_package(dir, package, topic)?
        };
        Ok(page.map_or(HelpResolution::NotFound, HelpResolution::Page))
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

    /// Return the metadata title for a prepared page, or an empty string when
    /// its metadata is absent or unreadable.
    pub fn title_for_page(&mut self, page: &PreparedHelpPage) -> String {
        self.package_index(&page.package, &page.package_dir, &page.help_key)
            .ok()
            .flatten()
            .and_then(|index| {
                index
                    .entries()
                    .find(|entry| entry.topic_key() == Some(page.help_key.as_str()))
                    .map(|entry| entry.title.as_str().unwrap_or_default().to_owned())
            })
            .unwrap_or_default()
    }

    fn matching_topics(
        &mut self,
        package: &str,
        dir: &Path,
        topic: &str,
        options: ResolveOptions,
    ) -> HarpResult<Vec<HelpTopic>> {
        match self.package_index(package, dir, topic) {
            Ok(Some(index)) => Ok(matching_entries(
                index,
                dir,
                package,
                topic,
                matches!(options.candidate_display, CandidateDisplay::Metadata),
            )),
            Ok(None) => Ok(Vec::new()),
            Err(error) if options.skip_broken_indexes => {
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
        options: ResolveOptions,
    ) -> HarpResult<HelpResolution> {
        let mut seen = HashSet::new();
        candidates.retain(|entry| match &entry.help_key {
            Some(key) => seen.insert((entry.package_dir.clone(), Some(key.clone()))),
            None if !options.deduplicate_keyless => true,
            None => seen.insert((entry.package_dir.clone(), None)),
        });
        match candidates.len() {
            0 => Ok(HelpResolution::NotFound),
            1 => {
                let candidate = &candidates[0];
                let page = Self::prepare_candidate(candidate)?;
                Ok(HelpResolution::Page(page))
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
