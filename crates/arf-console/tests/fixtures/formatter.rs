//! Test-only Air/Arity replacement: `--version` succeeds; Air accepts
//! `format --stdin-file-path arf-reprex.R --force`, while Arity accepts
//! `format -`, and both read stdin. `FORMATTER_FIXTURE_MODE=failure` makes
//! input `42` write a diagnostic to stderr and exit 17; `continuation` writes
//! `sum(1,` and rejects a standalone continuation fragment; `prefix-continuation`
//! writes a visible expression followed by `1 +`;
//! otherwise the fixture echoes input.

use std::io::{self, Read, Write};
use std::process;

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args == ["--version"] {
        println!("formatter test fixture");
        return;
    }

    let formatter = std::env::current_exe().ok().and_then(|path| {
        path.file_stem()
            .map(|name| name.to_string_lossy().into_owned())
    });
    let valid_arguments = match formatter.as_deref() {
        Some("air") => {
            args.as_slice() == ["format", "--stdin-file-path", "arf-reprex.R", "--force"]
        }
        Some("arity") => args.as_slice() == ["format", "-"],
        _ => false,
    };
    if !valid_arguments {
        process::exit(18);
    }
    if let Err(error) = format_stdin() {
        eprintln!("{error}");
        process::exit(1);
    }
}

fn format_stdin() -> io::Result<()> {
    let mode = std::env::var("FORMATTER_FIXTURE_MODE").unwrap_or_default();
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    match (mode.as_str(), input.as_slice()) {
        ("failure", b"42") => {
            eprintln!("synthetic formatter failure");
            process::exit(17);
        }
        ("continuation", b"42") => io::stdout().write_all(b"sum(1,"),
        ("continuation", b"2)") => {
            eprintln!("the continuation fragment must bypass the formatter");
            process::exit(19);
        }
        ("prefix-continuation", b"42") => io::stdout().write_all(b"cat('PREFIX\\n'); 1 +"),
        _ => io::stdout().write_all(&input),
    }
}
