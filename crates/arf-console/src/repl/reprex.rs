//! Reprex mode utilities.

use crossterm::{cursor, terminal::{self, ClearType}, ExecutableCommand};
use std::io::{self, Write};

/// Strip reprex output comment lines from input.
///
/// When pasting reprex output, lines starting with "#>" (the standard reprex output prefix)
/// would cause duplicate output if executed. This function removes those lines,
/// keeping only the actual R code.
///
/// # Examples
/// ```ignore
/// // Input (pasted from reprex):
/// x <- 1 + 1
/// x
/// #> [1] 2
///
/// // Output (after stripping):
/// x <- 1 + 1
/// x
/// ```
pub fn strip_reprex_output(input: &str) -> String {
    input
        .lines()
        .filter(|line| !line.starts_with("#>"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Clear the prompt from input lines in reprex mode and re-echo the source code.
/// This removes the prompt but keeps the input visible for clean reprex output.
///
/// - `original`: The original input (used to calculate how many lines to clear)
/// - `display`: The code to display (may be formatted differently from original)
pub fn clear_input_lines(original: &str, display: &str) {
    let mut stdout = io::stdout();

    // Count number of lines in the ORIGINAL input (what the user typed)
    let line_count = original.lines().count().max(1);

    // Move cursor up to the beginning of the prompt
    let _ = stdout.execute(cursor::MoveUp(line_count as u16));

    // Clear each line
    for _ in 0..line_count {
        let _ = stdout.execute(terminal::Clear(ClearType::CurrentLine));
        let _ = stdout.execute(cursor::MoveDown(1));
    }

    // Move back up to re-print the source code
    let _ = stdout.execute(cursor::MoveUp(line_count as u16));

    // Print the (possibly formatted) source code without prompt
    println!("{}", display);

    // Ensure all output is flushed
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_single_output_line() {
        let input = "x <- 1 + 1\nx\n#> [1] 2";
        let result = strip_reprex_output(input);
        assert_eq!(result, "x <- 1 + 1\nx");
    }

    #[test]
    fn test_strip_multiple_output_lines() {
        let input = "1:3\n#> [1] 1 2 3\nmean(1:3)\n#> [1] 2";
        let result = strip_reprex_output(input);
        assert_eq!(result, "1:3\nmean(1:3)");
    }

    #[test]
    fn test_no_output_lines() {
        let input = "x <- 1\ny <- 2\nz <- x + y";
        let result = strip_reprex_output(input);
        assert_eq!(result, input);
    }

    #[test]
    fn test_only_output_lines() {
        let input = "#> [1] 1\n#> [1] 2";
        let result = strip_reprex_output(input);
        assert_eq!(result, "");
    }

    #[test]
    fn test_empty_input() {
        let result = strip_reprex_output("");
        assert_eq!(result, "");
    }

    #[test]
    fn test_preserves_regular_comments() {
        // Regular R comments (# ) should NOT be stripped
        let input = "# This is a comment\nx <- 1\n#> [1] 1";
        let result = strip_reprex_output(input);
        assert_eq!(result, "# This is a comment\nx <- 1");
    }

    #[test]
    fn test_output_with_spaces_after_prefix() {
        // Real reprex output has space after #>
        let input = "x\n#>  [1] 1";
        let result = strip_reprex_output(input);
        assert_eq!(result, "x");
    }

    #[test]
    fn test_multiline_output() {
        // Large vector output spans multiple lines
        let input = "1:10\n#>  [1]  1  2  3  4  5  6  7  8  9 10";
        let result = strip_reprex_output(input);
        assert_eq!(result, "1:10");
    }

    #[test]
    fn test_preserves_hash_in_strings() {
        // #> inside a string should still be stripped if at line start
        // (but typically reprex would not produce this)
        let input = "\"#> not output\"\n#> [1] \"#> not output\"";
        let result = strip_reprex_output(input);
        assert_eq!(result, "\"#> not output\"");
    }
}
