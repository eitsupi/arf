//! Helpers for unit tests that need an isolated embedded R runtime.

use std::process::{Command, Stdio};
use std::sync::Mutex;

// Coordinate child environment inheritance with tests that change R's startup
// environment. Each R test then owns its runtime and counters in a fresh process.
pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Run exactly one ignored test in a child process, initializing R only there.
/// `qualified_name` includes the crate prefix from `module_path!()`.
pub(crate) fn with_r_in_subprocess(qualified_name: &str, test: impl FnOnce()) {
    const CHILD_TEST: &str = "ARF_HARP_R_TEST_CHILD";
    let (_, test_name) = qualified_name
        .split_once("::")
        .expect("test name should include its crate");

    let is_child = {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::var(CHILD_TEST).as_deref() == Ok(test_name)
    };
    if is_child {
        #[cfg(target_os = "linux")]
        {
            let library = arf_libr::find_r_library().expect("R should be installed");
            let directory = library.parent().expect("R library should have a directory");
            let search_path = std::env::var_os("LD_LIBRARY_PATH").unwrap_or_default();
            assert!(
                std::env::split_paths(&search_path).any(|path| path == directory),
                "LD_LIBRARY_PATH must include {} to load R packages",
                directory.display()
            );
        }

        // SAFETY: Only this exact test runs in the child process, and it is the
        // sole owner of R initialization and all subsequent R calls.
        unsafe { arf_libr::initialize_r_for_tests() }.expect("R should initialize");
        test();
        return;
    }

    let child = {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        Command::new(std::env::current_exe().expect("test executable should be available"))
            .args([
                "--exact",
                test_name,
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_TEST, test_name)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("R test child should start")
    };
    let output = child
        .wait_with_output()
        .expect("R test child should finish");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    print!("{stdout}");
    eprint!("{stderr}");
    assert!(output.status.success(), "R test child failed: {test_name}");
    // A misspelled test filter or an early R exit must not look like a pass.
    assert!(
        stdout.contains("1 passed;"),
        "R test did not pass: {test_name}"
    );
}
