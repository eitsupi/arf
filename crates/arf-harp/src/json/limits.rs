//! Resource limits and the bookkeeping shared by every conversion direction.

use super::error::{JsonError, JsonErrorKind, JsonPath, JsonPathSegment};

/// Deepest allowed nesting of arrays and objects.
///
/// A scalar document has depth 0, `[1]` has depth 1, `[[1]]` depth 2, and so
/// on. 64 is far deeper than any structured call argument needs while keeping
/// the recursion in both directions, and in R's own evaluator, shallow.
pub const MAX_DEPTH: usize = 64;

/// Largest number of values (scalars, arrays and objects, counted together)
/// in one document.
pub const MAX_NODES: usize = 1_000_000;

/// Largest total size, in UTF-8 bytes, of all strings and object keys in one
/// document.
pub const MAX_STRING_BYTES: usize = 16 * 1024 * 1024;

/// The largest magnitude of an integer-valued number that a double holds
/// exactly (2^53).
pub(super) const MAX_EXACT_INTEGER: u64 = 1 << 53;

/// Counts the values and string bytes seen so far against the limits.
#[derive(Debug, Default)]
pub(super) struct Budget {
    nodes: usize,
    string_bytes: usize,
}

impl Budget {
    /// Account for one more value.
    pub(super) fn count_node(&mut self) -> Result<(), JsonErrorKind> {
        if self.nodes >= MAX_NODES {
            return Err(JsonErrorKind::TooManyNodes { limit: MAX_NODES });
        }
        self.nodes += 1;
        Ok(())
    }

    /// How many more bytes of string data fit in the budget.
    pub(super) fn remaining_string_bytes(&self) -> usize {
        MAX_STRING_BYTES - self.string_bytes
    }

    /// Account for `bytes` more bytes of string or key data.
    pub(super) fn count_string(&mut self, bytes: usize) -> Result<(), JsonErrorKind> {
        match self.string_bytes.checked_add(bytes) {
            Some(total) if total <= MAX_STRING_BYTES => {
                self.string_bytes = total;
                Ok(())
            }
            _ => Err(JsonErrorKind::StringBytesExceeded {
                limit: MAX_STRING_BYTES,
            }),
        }
    }
}

/// Check that entering a container at nesting level `depth` (1 for the
/// outermost container) stays within [`MAX_DEPTH`].
pub(super) fn check_depth(depth: usize) -> Result<(), JsonErrorKind> {
    if depth > MAX_DEPTH {
        Err(JsonErrorKind::DepthExceeded { limit: MAX_DEPTH })
    } else {
        Ok(())
    }
}

/// The steps from the document root to the value being processed.
#[derive(Debug, Default)]
pub(super) struct PathStack {
    segments: Vec<JsonPathSegment>,
}

impl PathStack {
    pub(super) fn push_index(&mut self, index: usize) {
        self.segments.push(JsonPathSegment::Index(index));
    }

    pub(super) fn push_key(&mut self, key: &str) {
        self.segments.push(JsonPathSegment::Key(key.to_owned()));
    }

    pub(super) fn pop(&mut self) {
        self.segments.pop();
    }

    /// Snapshot the current location.
    pub(super) fn path(&self) -> JsonPath {
        JsonPath::from_segments(self.segments.clone())
    }

    /// Attach the current location to `kind`.
    pub(super) fn error(&self, kind: JsonErrorKind) -> JsonError {
        JsonError::new(self.path(), kind)
    }
}
