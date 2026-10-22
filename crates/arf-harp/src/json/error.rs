//! Error types for the R/JSON conversion.

use std::fmt;
use thiserror::Error;

/// One step of a [`JsonPath`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonPathSegment {
    /// An object member, identified by its key.
    Key(String),
    /// An array element, identified by its zero-based position.
    Index(usize),
}

/// The location of a value inside a document being converted.
///
/// The path is rendered as a JSON Pointer (RFC 6901): the empty string
/// is the document root and every other step is introduced by `/`. The
/// characters `~` and `/` inside a key are written as `~0` and `~1`, so a
/// key such as `"a/b"` can never be mistaken for two steps. The same
/// notation is used when the document is an R value: list elements are
/// addressed by name (object) or position (array), exactly as they would be
/// after conversion.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JsonPath {
    segments: Vec<JsonPathSegment>,
}

impl JsonPath {
    /// The path of the document root.
    pub fn root() -> Self {
        Self::default()
    }

    /// Build a path from its steps.
    pub fn from_segments(segments: Vec<JsonPathSegment>) -> Self {
        Self { segments }
    }

    /// The steps leading from the root to the value, outermost first.
    pub fn segments(&self) -> &[JsonPathSegment] {
        &self.segments
    }

    /// Whether this is the document root.
    pub fn is_root(&self) -> bool {
        self.segments.is_empty()
    }

    /// Render the path as a JSON Pointer.
    pub fn to_pointer(&self) -> String {
        let mut pointer = String::new();
        for segment in &self.segments {
            pointer.push('/');
            match segment {
                JsonPathSegment::Index(index) => pointer.push_str(&index.to_string()),
                JsonPathSegment::Key(key) => {
                    for ch in key.chars() {
                        match ch {
                            '~' => pointer.push_str("~0"),
                            '/' => pointer.push_str("~1"),
                            other => pointer.push(other),
                        }
                    }
                }
            }
        }
        pointer
    }
}

impl fmt::Display for JsonPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_pointer())
    }
}

/// Why a conversion failed.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum JsonErrorKind {
    /// Arrays/objects are nested deeper than [`MAX_DEPTH`](super::MAX_DEPTH).
    #[error("nesting is deeper than the limit of {limit} levels")]
    DepthExceeded { limit: usize },

    /// The document has more values than [`MAX_NODES`](super::MAX_NODES).
    #[error("the document has more than {limit} values")]
    TooManyNodes { limit: usize },

    /// Strings and keys together exceed [`MAX_STRING_BYTES`](super::MAX_STRING_BYTES).
    #[error("strings and keys total more than {limit} bytes")]
    StringBytesExceeded { limit: usize },

    /// An R list has two elements with the same name (after conversion to UTF-8).
    #[error("duplicate object key")]
    DuplicateKey,

    /// A string contains U+0000, which an R string cannot hold.
    #[error("string contains a NUL character, which R strings cannot hold")]
    NulInString,

    /// An integer-valued JSON number does not fit a double exactly.
    #[error("integer is outside +/-2^53 and cannot be represented exactly as a double")]
    IntegerOutOfRange,

    /// A number is NaN or infinite (including a literal that overflows a double).
    #[error("number is not finite")]
    NonFiniteNumber,

    /// An R value of a type that has no JSON counterpart.
    #[error("R values of type {type_name} cannot be converted to JSON")]
    UnsupportedType { type_name: String },

    /// An atomic vector whose length is not 1.
    #[error("atomic vector of length {length} cannot be converted; expected length 1")]
    InvalidLength { length: usize },

    /// NA in a logical, integer, double or character vector.
    #[error("NA cannot be converted to JSON")]
    MissingValue,

    /// A value carrying a class attribute.
    #[error("values with a class attribute cannot be converted")]
    ClassAttribute,

    /// A `names` attribute that is not a character vector matching the list length.
    #[error("the names attribute is malformed")]
    MalformedNames,

    /// A list element is named NA.
    #[error("NA is not allowed as a name")]
    NaName,

    /// A string has the `bytes` encoding, which carries no character set.
    #[error("string is marked as bytes and cannot be converted to UTF-8")]
    BytesEncoding,

    /// A string could not be converted to valid UTF-8.
    #[error("string cannot be converted to valid UTF-8")]
    InvalidEncoding,

    /// R signalled an error (or the evaluation was interrupted) mid-conversion.
    #[error("R signalled an error or interrupt during the conversion")]
    RError,

    /// The embedded R library is not available.
    #[error("R is not available: {0}")]
    RUnavailable(String),
}

/// A failed conversion: what went wrong and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonError {
    path: JsonPath,
    kind: JsonErrorKind,
}

impl JsonError {
    pub(crate) fn new(path: JsonPath, kind: JsonErrorKind) -> Self {
        Self { path, kind }
    }

    pub(crate) fn at_root(kind: JsonErrorKind) -> Self {
        Self::new(JsonPath::root(), kind)
    }

    /// Where in the document the failure happened.
    pub fn path(&self) -> &JsonPath {
        &self.path
    }

    /// Why the conversion failed.
    pub fn kind(&self) -> &JsonErrorKind {
        &self.kind
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} (at JSON Pointer \"{}\")", self.kind, self.path)
    }
}

impl std::error::Error for JsonError {}
