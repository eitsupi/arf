//! JSON output helpers: pretty-print to terminals and compact output to pipes.

use anyhow::{Context, Result};
use serde::Serialize;
use std::io::Write;

/// Write JSON to `writer`, using pretty-printing when `pretty` is true.
///
/// This function does not flush the writer; flushing remains the caller's
/// responsibility.
pub(crate) fn write_json<T, W>(writer: &mut W, value: &T, pretty: bool) -> Result<()>
where
    T: Serialize + ?Sized,
    W: Write + ?Sized,
{
    let json = if pretty {
        serde_json::to_string_pretty(value).context("Failed to serialize JSON")?
    } else {
        serde_json::to_string(value).context("Failed to serialize JSON")?
    };

    writer
        .write_all(json.as_bytes())
        .context("Failed to write JSON")?;
    Ok(())
}

/// Print JSON to stdout, selecting pretty or compact output based on whether
/// stdout is a terminal. Writes a trailing newline but does not flush stdout;
/// flushing remains the caller's responsibility.
pub(crate) fn print_json<T>(value: &T) -> Result<()>
where
    T: Serialize + ?Sized,
{
    let stdout = std::io::stdout();
    let pretty = std::io::IsTerminal::is_terminal(&stdout);
    let mut stdout = stdout.lock();
    write_json(&mut stdout, value, pretty)?;
    writeln!(stdout).context("Failed to write JSON newline")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::write_json;
    use serde_json::Value;

    #[test]
    fn compact_json_is_single_line_and_valid() {
        let value = serde_json::json!({"name": "arf", "items": [1, 2, 3]});
        let mut output = Vec::new();

        write_json(&mut output, &value, false).unwrap();

        assert!(!output.contains(&b'\n'));
        assert!(serde_json::from_slice::<Value>(&output).is_ok());
    }

    #[test]
    fn pretty_json_is_multiple_lines_and_valid() {
        let value = serde_json::json!({"name": "arf", "items": [1, 2, 3]});
        let mut output = Vec::new();

        write_json(&mut output, &value, true).unwrap();

        assert!(output.contains(&b'\n'));
        assert!(serde_json::from_slice::<Value>(&output).is_ok());
    }
}
