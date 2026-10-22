//! Subscription patterns and per-connection subscription sets.

use std::collections::HashSet;
use std::fmt;

use super::event::{Topic, TopicError, validate_prefix};

/// A subscription pattern: an exact topic or a `prefix.*` wildcard.
///
/// `foo.*` matches every topic starting with `foo.` (`foo.bar`,
/// `foo.bar.baz`) but not `foo` itself nor `foobar`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SubscriptionPattern {
    Exact(String),
    /// Stored with the trailing dot, e.g. `foo.`.
    Prefix(String),
}

impl SubscriptionPattern {
    /// Parse a pattern. Subscribers may name any topic, reserved or not.
    pub fn parse(pattern: &str) -> Result<Self, TopicError> {
        match pattern.strip_suffix(".*") {
            Some(prefix) => {
                validate_prefix(prefix)?;
                Ok(Self::Prefix(format!("{prefix}.")))
            }
            None => Ok(Self::Exact(Topic::internal(pattern)?.as_str().to_owned())),
        }
    }

    pub fn matches(&self, topic: &Topic) -> bool {
        match self {
            Self::Exact(name) => name == topic.as_str(),
            Self::Prefix(prefix) => topic.as_str().starts_with(prefix.as_str()),
        }
    }
}

impl fmt::Display for SubscriptionPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exact(name) => f.write_str(name),
            Self::Prefix(prefix) => write!(f, "{prefix}*"),
        }
    }
}

/// The set of patterns one connection is subscribed to.
#[derive(Debug, Default)]
pub(super) struct SubscriptionSet {
    exact: HashSet<String>,
    /// Prefixes including the trailing dot.
    prefixes: HashSet<String>,
}

impl SubscriptionSet {
    /// Add a pattern; returns whether it was newly added.
    pub(super) fn insert(&mut self, pattern: SubscriptionPattern) -> bool {
        match pattern {
            SubscriptionPattern::Exact(name) => self.exact.insert(name),
            SubscriptionPattern::Prefix(prefix) => self.prefixes.insert(prefix),
        }
    }

    pub(super) fn contains(&self, pattern: &SubscriptionPattern) -> bool {
        match pattern {
            SubscriptionPattern::Exact(name) => self.exact.contains(name),
            SubscriptionPattern::Prefix(prefix) => self.prefixes.contains(prefix),
        }
    }

    /// Remove a pattern; returns whether it was present.
    pub(super) fn remove(&mut self, pattern: &SubscriptionPattern) -> bool {
        match pattern {
            SubscriptionPattern::Exact(name) => self.exact.remove(name),
            SubscriptionPattern::Prefix(prefix) => self.prefixes.remove(prefix),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.exact.len() + self.prefixes.len()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(super) fn matches(&self, topic: &Topic) -> bool {
        let name = topic.as_str();
        if self.exact.contains(name) {
            return true;
        }
        if self.prefixes.is_empty() {
            return false;
        }
        // Every proper prefix of a topic that ends right after a dot.
        name.match_indices('.')
            .any(|(i, _)| self.prefixes.contains(&name[..=i]))
    }
}
