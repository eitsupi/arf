//! Portable RHOME launcher fixture; never starts or embeds R.

fn main() {
    if std::env::args().nth(1).as_deref() != Some("RHOME") {
        std::process::exit(2);
    }
    if std::env::var_os("R_HOME").is_some() {
        eprintln!("inherited R_HOME reached the probe");
        std::process::exit(3);
    }
    if let Ok(home) = std::env::var("ARF_TEST_RHOME_OUTPUT") {
        println!("WARNING: harmless wrapper warning\n{home}");
    }
}
