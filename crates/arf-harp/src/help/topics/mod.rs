//! Help, vignette, and demo topic indexes read from installed package metadata.

use super::HelpTopic;
use crate::lib_paths::installed_package_dirs;
use rd_helpdb::{DemoIndex, HelpTopicEntry, HelpTopicIndex, VignetteIndex, read_rds_file};

/// Get help, vignette, and demo topics from explicit library paths.
///
/// This function only reads the supplied filesystem paths and metadata; it
/// does not evaluate R. The paths are searched in order, with the first
/// installed copy of a package taking precedence. Missing or unreadable
/// metadata sources are skipped.
///
/// Rd topic metadata, vignette indexes, and demo indexes are read independently.
pub fn get_help_topics_from_paths(paths: &[String]) -> Vec<HelpTopic> {
    let mut topics = Vec::new();
    for (package, package_dir) in installed_package_dirs(paths) {
        topics.extend(read_package_topics(&package, &package_dir));
    }
    topics
}

/// Get only Rd help topics from explicit library paths, optionally limited to
/// one package name.
///
/// Like [`get_help_topics_from_paths`], unreadable Rd metadata is skipped so
/// one broken package does not hide the others.
pub fn get_rd_help_topics_from_paths(
    paths: &[String],
    package_filter: Option<&str>,
) -> Vec<HelpTopic> {
    installed_package_dirs(paths)
        .into_iter()
        .filter(|(package, _)| package_filter.is_none_or(|filter| package == filter))
        .flat_map(|(package, package_dir)| read_rd_help_topics(&package, &package_dir))
        .collect()
}

fn read_rd_help_topics(package: &str, package_dir: &std::path::Path) -> Vec<HelpTopic> {
    match HelpTopicIndex::read_installed(package_dir) {
        Ok(Some(index)) => index
            .entries()
            .filter_map(|entry| project_help_topic_entry(package, package_dir, entry))
            .collect(),
        Ok(None) => Vec::new(),
        Err(error) => {
            log::debug!(
                "Skipping unreadable Rd help metadata in {}: {error}",
                package_dir.display()
            );
            Vec::new()
        }
    }
}

fn read_package_topics(package: &str, package_dir: &std::path::Path) -> Vec<HelpTopic> {
    // Each metadata source is independent of compiled help files and of the
    // other metadata indexes. A malformed source should not hide the rest.
    let mut topics = read_rd_help_topics(package, package_dir);

    if let Some(object) = read_metadata_index(package_dir.join("Meta/vignette.rds")) {
        match VignetteIndex::from_object(&object) {
            Ok(index) => topics.extend(index.entries().map(|entry| HelpTopic {
                package: package.to_owned(),
                package_dir: package_dir.to_owned(),
                topic: vignette_topic(entry),
                aliases: Vec::new(),
                help_key: None,
                title: entry.title.clone(),
                entry_type: "vignette".to_string(),
            })),
            Err(error) => log::debug!(
                "Skipping malformed vignette metadata in {}: {error}",
                package_dir.display()
            ),
        }
    }

    if let Some(object) = read_metadata_index(package_dir.join("Meta/demo.rds")) {
        match DemoIndex::from_object(&object) {
            Ok(index) => topics.extend(index.entries().map(|entry| HelpTopic {
                package: package.to_owned(),
                package_dir: package_dir.to_owned(),
                topic: entry.name.clone(),
                aliases: Vec::new(),
                help_key: None,
                title: entry.title.clone(),
                entry_type: "demo".to_string(),
            })),
            Err(error) => log::debug!(
                "Skipping malformed demo metadata in {}: {error}",
                package_dir.display()
            ),
        }
    }

    topics
}

fn read_metadata_index(path: std::path::PathBuf) -> Option<rd_rds::RObject> {
    match read_rds_file(&path) {
        Ok(object) => Some(object),
        Err(rd_helpdb::Error::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            None
        }
        Err(error) => {
            log::debug!(
                "Skipping unreadable help metadata {}: {error}",
                path.display()
            );
            None
        }
    }
}

fn project_help_topic_entry(
    package: &str,
    package_dir: &std::path::Path,
    entry: &HelpTopicEntry,
) -> Option<HelpTopic> {
    let aliases: Vec<String> = entry.aliases.iter().flatten().cloned().collect();
    let topic = entry
        .name
        .as_str()
        .filter(|name| aliases.iter().any(|alias| alias == name))
        .or_else(|| entry.aliases.first().and_then(Option::as_deref))?;

    Some(HelpTopic {
        package: package.to_owned(),
        package_dir: package_dir.to_owned(),
        topic: topic.to_owned(),
        aliases,
        help_key: entry
            .topic_key()
            .filter(|key| !key.is_empty())
            .map(str::to_owned),
        title: entry.title.as_str().unwrap_or("").to_owned(),
        entry_type: "help".to_string(),
    })
}

// Mirror R's vignette-topic resolution from the index's filename fields.
fn vignette_topic(entry: &rd_helpdb::VignetteEntry) -> String {
    let (filename, from_file) = if !entry.r.is_empty() {
        (&entry.r, false)
    } else if !entry.pdf.is_empty() {
        (&entry.pdf, false)
    } else {
        (&entry.file, true)
    };
    let filename = if from_file {
        filename.rsplit(['/', '\\']).next().unwrap_or(filename)
    } else {
        filename
    };
    filename
        .rsplit_once('.')
        .map_or_else(|| filename.to_owned(), |(stem, _)| stem.to_owned())
}

#[cfg(test)]
mod tests;
