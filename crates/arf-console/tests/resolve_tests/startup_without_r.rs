use super::fixtures::RLessEnvironment;
use crate::common::wait_for_child_exit;
use std::process::Stdio;
use std::time::Duration;

/// This covers initialization-failure fallback, not PATH discovery.
#[test]
#[cfg(not(windows))]
fn startup_survives_without_r_on_path() {
    let environment = RLessEnvironment::new();
    let mut child = environment
        .startup_command()
        .args(["--no-banner", "--no-history"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let timeout = Duration::from_secs(10);
    if !wait_for_child_exit(&mut child, timeout, Duration::from_millis(50)).unwrap() {
        let output = child.wait_with_output().unwrap();
        panic!(
            "arf did not exit within {timeout:?}; stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = child.wait_with_output().unwrap();

    assert!(
        output.status.success(),
        "arf should start and exit cleanly without a usable R library: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("R evaluation will not be available"));
}
