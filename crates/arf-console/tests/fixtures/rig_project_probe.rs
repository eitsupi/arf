//! Native fake `rig` and project R wrapper for provider integration tests.

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process;

fn record(key: &str, value: impl AsRef<[u8]>) {
    if let Some(path) = env::var_os(key) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        file.write_all(value.as_ref()).unwrap();
        file.write_all(b"\n").unwrap();
    }
}

fn main() {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "RHOME") {
        if env::var_os("R_HOME").is_some() {
            eprintln!("inherited R_HOME reached the project wrapper");
            process::exit(3);
        }
        record(
            "ARF_TEST_RIG_WRAPPER_CWD",
            env::current_dir()
                .unwrap()
                .as_os_str()
                .to_string_lossy()
                .as_bytes(),
        );
        let home = env::var_os("ARF_TEST_RIG_R_HOME").expect("fake R_HOME path");
        println!("{}", PathBuf::from(home).display());
        return;
    }

    if args
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>()
        != ["proj", "status", "--json"]
    {
        eprintln!("unexpected fake rig arguments: {args:?}");
        process::exit(2);
    }
    record(
        "ARF_TEST_RIG_STATUS_CWD",
        env::current_dir()
            .unwrap()
            .as_os_str()
            .to_string_lossy()
            .as_bytes(),
    );
    if env::var_os("ARF_TEST_RIG_STATUS_FAIL").is_some() {
        process::exit(1);
    }
    let status =
        fs::read(env::var_os("ARF_TEST_RIG_STATUS_FILE").expect("status fixture path")).unwrap();
    print!("{}", String::from_utf8(status).unwrap());
}
