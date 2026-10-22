//! Conversion between R values and [`serde_json::Value`].
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
//! attribute (factors, dates and data frames included); an `NA` or repeated
//! name in a named list. The empty string is a valid name. A `names`
//! attribute on a length-1 atomic vector is ignored, since the JSON scalar has
//! no place for it. Attributes other than `class` and `names` (`dim`,
//! `levels`, ...) are ignored too, not rejected.
//!
//! Going from JSON to R, these are errors: a string containing U+0000, and an
//! integer-valued number outside +/-2^53, which a double cannot hold exactly.
//! A floating-point number must be finite. Only what the [`serde_json::Value`]
//! still records can be checked: if `serde_json` already rounded an integer in
//! the original text to a double (for example one beyond 64 bits), the
//! rounding cannot be detected here.
//!
//! # Objects
//!
//! A JSON object is an unordered set of members. Neither direction promises to
//! keep order: [`serde_json::Map`] is sorted by key (the crate's
//! `preserve_order` feature is not enabled), so a named R list may come back
//! in a different order. Use an array when order matters.
//!
//! The two directions differ on repeated keys. A [`serde_json::Value`] cannot
//! hold the same key twice, and `serde_json` keeps the last one when parsing,
//! so a duplicate in incoming JSON is accepted. A named R list can repeat a
//! name, and silently dropping one element would lose data, so converting such
//! a list to JSON is an error. Names are compared after conversion to UTF-8.
//!
//! # Strings and encodings
//!
//! Strings from R are translated to UTF-8 with `Rf_translateCharUTF8`, so
//! UTF-8 and Latin-1 marked strings convert exactly. A string in the native
//! encoding of a non-UTF-8 locale is translated by R, which replaces bytes it
//! cannot convert with `<xx>` escapes instead of failing; such a conversion is
//! not guaranteed to be exact or reversible. Strings marked as `bytes`, and
//! strings that are not valid UTF-8 after translation, are errors. Strings
//! going to R are created with an explicit UTF-8 mark.
//!
//! # Limits
//!
//! Documents are limited by nesting depth ([`MAX_DEPTH`]), number of values
//! ([`MAX_NODES`]) and total string and key size ([`MAX_STRING_BYTES`]).
//! Exceeding one is an error, not a truncation.
//!
//! # Errors and R state
//!
//! Every error is a [`JsonError`] naming the failing element by JSON Pointer
//! and giving a [`JsonErrorKind`]. A failed conversion never leaves protected
//! objects behind, so the next call works normally.
//!
//! R API calls that can raise an R error run under `R_ToplevelExec`. When one
//! does, R still prints its default error message to the console, and the
//! conversion fails with [`JsonErrorKind::RError`]. A user interrupt during the
//! conversion is reported the same way.
//!
//! All functions that touch R values must be called on the R thread.

mod error;
mod from_r;
mod limits;
mod r_call;
mod to_r;

#[cfg(test)]
mod r_tests;
#[cfg(test)]
mod tests;

pub use error::{JsonError, JsonErrorKind, JsonPath, JsonPathSegment};
pub use from_r::{r_to_json, r_to_json_text};
pub use limits::{MAX_DEPTH, MAX_NODES, MAX_STRING_BYTES};
pub use to_r::json_to_r;
