//! Portable RHOME launcher fixture; never starts or embeds R.

fn main() {
    if std::env::args().nth(1).as_deref() != Some("RHOME") {
        std::process::exit(2);
    }
    if let Some(path) = std::env::var_os("ARF_TEST_RHOME_COUNT") {
        let count = std::fs::read_to_string(&path)
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        std::fs::write(path, (count + 1).to_string()).unwrap();
    }
    if std::env::var_os("R_HOME").is_some() {
        eprintln!("inherited R_HOME reached the probe");
        std::process::exit(3);
    }
    match std::env::var("ARF_TEST_RHOME_MODE").as_deref() {
        Ok("stdout" | "stderr") => {
            use std::io::Write;
            let bytes = vec![b'x'; 65 * 1024];
            if std::env::var("ARF_TEST_RHOME_MODE").unwrap() == "stdout" {
                let _ = std::io::stdout().write_all(&bytes);
            } else {
                let _ = std::io::stderr().write_all(&bytes);
            }
            std::thread::sleep(std::time::Duration::from_secs(60));
            return;
        }
        Ok("hang") => {
            std::thread::sleep(std::time::Duration::from_secs(60));
            return;
        }
        _ => {}
    }
    if let Ok(home) = std::env::var("ARF_TEST_RHOME_OUTPUT") {
        println!("WARNING: harmless wrapper warning\n{home}");
    }
}
