//! Event value types: topics, execution identifiers and the event itself.

use std::fmt;

use serde::Serialize;
use serde_json::Value;

/// Prefix reserved for events that originate inside arf itself.
const RESERVED_PREFIX: &str = "arf.";

/// Longest accepted topic or subscription pattern, in bytes.
pub(super) const MAX_TOPIC_BYTES: usize = 128;

/// Fixed allowance added to every event's size for the envelope fields
/// (`seq`, `execution` and the surrounding JSON punctuation).
const ENVELOPE_OVERHEAD_BYTES: usize = 96;

/// Why a topic or subscription pattern was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicError {
    Empty,
    TooLong {
        len: usize,
        max: usize,
    },
    /// Segments (separated by `.`) must be non-empty.
    EmptySegment,
    /// Only ASCII letters, digits, `_` and `-` are allowed within a segment.
    InvalidCharacter(char),
    /// The topic is in the `arf.` namespace, which only arf itself may publish.
    Reserved,
}

impl fmt::Display for TopicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "topic is empty"),
            Self::TooLong { len, max } => {
                write!(f, "topic is {len} bytes long, the maximum is {max}")
            }
            Self::EmptySegment => write!(f, "topic contains an empty segment"),
            Self::InvalidCharacter(c) => write!(f, "topic contains invalid character {c:?}"),
            Self::Reserved => write!(f, "the arf. topic namespace is reserved"),
        }
    }
}

impl std::error::Error for TopicError {}

/// Validate a dot-separated name made of non-empty segments.
fn validate_name(name: &str) -> Result<(), TopicError> {
    if name.is_empty() {
        return Err(TopicError::Empty);
    }
    if name.len() > MAX_TOPIC_BYTES {
        return Err(TopicError::TooLong {
            len: name.len(),
            max: MAX_TOPIC_BYTES,
        });
    }
    for segment in name.split('.') {
        if segment.is_empty() {
            return Err(TopicError::EmptySegment);
        }
        if let Some(c) = segment
            .chars()
            .find(|c| !(c.is_ascii_alphanumeric() || *c == '_' || *c == '-'))
        {
            return Err(TopicError::InvalidCharacter(c));
        }
    }
    Ok(())
}

/// Whether `name` (already validated) lies in the arf-reserved namespace.
fn is_reserved_name(name: &str) -> bool {
    name.starts_with(RESERVED_PREFIX)
}

/// Validate the prefix part of a `prefix.*` pattern (without the `.*`).
pub(super) fn validate_prefix(prefix: &str) -> Result<(), TopicError> {
    validate_name(prefix)
}

/// A validated event topic.
///
/// Topics are dot-separated names such as `arf.execution.finished`. Names
/// starting with `arf.` are reserved for arf itself. The two constructors
/// make the origin explicit at the call site: code that forwards names chosen
/// by R code or other external parties must use [`Topic::external`], which
/// refuses reserved names.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Topic(String);

impl Topic {
    /// Parse a topic chosen by arf itself. Reserved names are allowed.
    pub fn internal(name: &str) -> Result<Self, TopicError> {
        validate_name(name)?;
        Ok(Self(name.to_owned()))
    }

    /// Parse a topic chosen by an external party (R code, clients).
    ///
    /// Fails with [`TopicError::Reserved`] for names in the `arf.` namespace.
    pub fn external(name: &str) -> Result<Self, TopicError> {
        validate_name(name)?;
        if is_reserved_name(name) {
            return Err(TopicError::Reserved);
        }
        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this topic is in the arf-reserved namespace.
    pub fn is_reserved(&self) -> bool {
        is_reserved_name(&self.0)
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identifier of the execution an event belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct ExecutionId(pub u64);

/// An event as delivered to subscribers.
///
/// Serializes to `{"seq", "topic", "data", "execution"}`. `seq` increases
/// strictly within a session in publication order; it exists only to compare
/// order, and gaps do not imply lost events.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Event {
    pub seq: u64,
    pub topic: Topic,
    pub data: Value,
    pub execution: Option<ExecutionId>,
    #[serde(skip)]
    size: usize,
}

impl Event {
    pub(super) fn new(
        seq: u64,
        topic: Topic,
        data: Value,
        execution: Option<ExecutionId>,
        data_len: usize,
    ) -> Self {
        let size = event_size(topic.as_str().len(), data_len);
        Self {
            seq,
            topic,
            data,
            execution,
            size,
        }
    }

    /// Approximate encoded size in bytes, used for all byte accounting.
    pub fn size(&self) -> usize {
        self.size
    }
}

/// Size accounted for an event with the given topic length and encoded data length.
pub(super) fn event_size(topic_len: usize, data_len: usize) -> usize {
    ENVELOPE_OVERHEAD_BYTES
        .saturating_add(topic_len)
        .saturating_add(data_len)
}

/// Encoded size of `value` as JSON, or `None` once it is known to exceed `limit`.
///
/// Serialization is abandoned as soon as the limit is crossed, so an oversized
/// value is never fully rendered.
pub(super) fn json_len_within(value: &Value, limit: usize) -> Option<usize> {
    struct Counter {
        len: usize,
        limit: usize,
    }
    impl std::io::Write for Counter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.len += buf.len();
            if self.len > self.limit {
                return Err(std::io::Error::other("size limit exceeded"));
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter { len: 0, limit };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.len)
}
