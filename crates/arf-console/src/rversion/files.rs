//! Readers for R version values stored in project files.

use std::fmt;
use std::fs::{self, File};
use std::io;
use std::io::{BufRead, Read};
use std::path::Path;

/// An error returned when reading a version from a TOML key.
#[derive(Debug)]
pub enum TomlKeyError {
    /// The TOML file could not be read.
    Read(io::Error),
    /// The TOML document could not be parsed.
    Parse(toml::de::Error),
    /// The requested key path does not exist.
    MissingKey(String),
    /// The requested key does not contain a string.
    NotString(String),
}

/// An error returned when reading an R version from a JSON key.
#[derive(Debug)]
pub enum JsonKeyError {
    /// The JSON file could not be read.
    Read(io::Error),
    /// The JSON document could not be parsed.
    Parse(serde_json::Error),
    /// The requested key path does not exist.
    MissingKey(String),
    /// The requested key does not contain a string.
    NotString(String),
}

impl fmt::Display for JsonKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "failed to read JSON file: {error}"),
            Self::Parse(_) => formatter.write_str("failed to parse JSON"),
            Self::MissingKey(key) => write!(formatter, "JSON key not found: {key}"),
            Self::NotString(key) => write!(formatter, "JSON key is not a string: {key}"),
        }
    }
}

impl std::error::Error for JsonKeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::MissingKey(_) | Self::NotString(_) => None,
        }
    }
}

impl JsonKeyError {
    /// Return whether this error means that the configured file is absent.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Read(error) if error.kind() == io::ErrorKind::NotFound)
    }
}

impl fmt::Display for TomlKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "failed to read TOML file: {error}"),
            Self::Parse(_) => formatter.write_str("failed to parse TOML"),
            Self::MissingKey(key) => write!(formatter, "TOML key not found: {key}"),
            Self::NotString(key) => write!(formatter, "TOML key is not a string: {key}"),
        }
    }
}

impl std::error::Error for TomlKeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::Parse(error) => Some(error),
            Self::MissingKey(_) | Self::NotString(_) => None,
        }
    }
}

impl TomlKeyError {
    /// Return whether this error means that the configured file is absent.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Read(error) if error.kind() == io::ErrorKind::NotFound)
    }
}

/// The maximum length of the trimmed value read from a version file.
pub(super) const MAX_VERSION_FILE_VALUE_LENGTH: usize = 256;

/// Read the first non-empty version specification from a plain text file.
pub fn read_version_file(path: &Path) -> io::Result<String> {
    let file = File::open(path)?;
    let mut reader = io::BufReader::new(file);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader
            .by_ref()
            // Limit + 1 for the line's own `\n`, plus 1 to recognize `\r\n` before judging length.
            .take((MAX_VERSION_FILE_VALUE_LENGTH + 2) as u64)
            .read_line(&mut line)?;
        if bytes_read == 0 {
            return Ok(String::new());
        }

        // Reject rather than truncate: truncation could turn an invalid value into a valid one.
        if bytes_read > MAX_VERSION_FILE_VALUE_LENGTH && !line.ends_with('\n') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("version file value exceeds {MAX_VERSION_FILE_VALUE_LENGTH} bytes"),
            ));
        }

        let value = line.trim();
        if value.is_empty() {
            continue;
        }

        // Reject rather than truncate: truncation could turn an invalid value into a valid one.
        if value.len() > MAX_VERSION_FILE_VALUE_LENGTH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("version file value exceeds {MAX_VERSION_FILE_VALUE_LENGTH} bytes"),
            ));
        }
        return Ok(value.to_owned());
    }
}

/// Read a string value from a dot-separated key path in a TOML file.
pub fn read_toml_key(path: &Path, key: &str) -> Result<String, TomlKeyError> {
    let contents = fs::read_to_string(path).map_err(TomlKeyError::Read)?;
    let document = toml::from_str::<toml::Value>(&contents).map_err(TomlKeyError::Parse)?;

    let mut value = &document;
    for component in key.split('.') {
        if component.is_empty() {
            return Err(TomlKeyError::MissingKey(key.to_owned()));
        }
        value = value
            .get(component)
            .ok_or_else(|| TomlKeyError::MissingKey(key.to_owned()))?;
    }

    value
        .as_str()
        .map(|version| version.trim().to_owned())
        .ok_or_else(|| TomlKeyError::NotString(key.to_owned()))
}

/// Read a string value from a dot-separated key path in a JSON file.
pub fn read_json_key(path: &Path, key: &str) -> Result<String, JsonKeyError> {
    let contents = fs::read_to_string(path).map_err(JsonKeyError::Read)?;
    let document =
        serde_json::from_str::<serde_json::Value>(&contents).map_err(JsonKeyError::Parse)?;

    let mut value = &document;
    for component in key.split('.') {
        if component.is_empty() {
            return Err(JsonKeyError::MissingKey(key.to_owned()));
        }
        value = value
            .as_object()
            .and_then(|object| object.get(component))
            .ok_or_else(|| JsonKeyError::MissingKey(key.to_owned()))?;
    }

    value
        .as_str()
        .map(|version| version.trim().to_owned())
        .ok_or_else(|| JsonKeyError::NotString(key.to_owned()))
}
