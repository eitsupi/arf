//! Conversion between R values and JSON values.
//!
//! This is not a general-purpose serializer and does not try to match
//! `jsonlite`. It defines a small mapping with no ambiguity, meant for the
//! arguments and results of functions called across a process boundary.
//!
//! # Mapping
//!
//! | JSON    | R                                                          |
//! |---------|------------------------------------------------------------|
//! | `null`  | `NULL`                                                     |
//! | boolean | length-1 `logical`                                         |
//! | number  | length-1 `double` (a length-1 `integer` is accepted from R) |
//! | string  | length-1 `character`, always UTF-8                         |
//! | array   | unnamed `list` (`[]` is `list()`)                          |
//! | object  | named `list` (`{}` is a zero-length list *with* a zero-length `names` attribute) |
//!
//! Going from R to JSON, these are errors: an atomic vector whose length is
//! not 1; `NA` of any type, `NaN` and infinities; `raw`, `complex`,
//! environments, functions and every other type; any value with a `class`
//! attribute (factors, dates and data frames included); any value with an
//! attribute other than `names`; an `NA` or repeated name in a named list.
//! The empty string is a valid name. A `names` attribute on a length-1 atomic
//! vector is ignored, since the JSON scalar has no place for it.
//!
//! Going from JSON to R, these are errors: a string containing U+0000; an
//! integer-valued number outside +/-2^53, which a double cannot hold exactly;
//! a repeated key in an object. Duplicate keys are detected while the text is
//! parsed, because a [`serde_json::Value`] no longer remembers them.
//!
//! Object member order follows [`serde_json::Map`], which is sorted by key
//! unless that crate's `preserve_order` feature is enabled. A named R list
//! therefore does not necessarily keep its element order through a trip via
//! [`serde_json::Value`].
//!
//! # Limits
//!
//! Documents are limited by nesting depth ([`MAX_DEPTH`]), number of values
//! ([`MAX_NODES`]), total string and key size ([`MAX_STRING_BYTES`]) and, for
//! text, length ([`MAX_TEXT_BYTES`]). Exceeding one is an error, not a
//! truncation.
//!
//! # Errors and R state
//!
//! Every error is a [`JsonError`] naming the failing element by JSON Pointer
//! and giving a [`JsonErrorKind`]. A failed conversion never leaves protected
//! objects behind, so the next call works normally.
//!
//! All functions that touch R values must be called on the R thread.

mod error;
mod from_r;
mod limits;
mod parse;
mod r_call;
mod to_r;

#[cfg(test)]
mod r_tests;
#[cfg(test)]
mod tests;

pub use error::{JsonError, JsonErrorKind, JsonPath, JsonPathSegment};
pub use from_r::{r_to_json, r_to_json_text};
pub use limits::{MAX_DEPTH, MAX_NODES, MAX_STRING_BYTES, MAX_TEXT_BYTES};
pub use to_r::{json_text_to_r, json_to_r};

/// Parse JSON text into a [`serde_json::Value`] under the same rules as
/// [`json_text_to_r`]: strict RFC 8259 syntax, the crate's limits, no
/// duplicate object keys and no integer beyond +/-2^53.
///
/// This does not need R. A string containing U+0000 parses here; it is
/// rejected when the value is converted to R.
pub fn parse_json_text(text: &str) -> Result<serde_json::Value, JsonError> {
    parse::parse(text)
}
