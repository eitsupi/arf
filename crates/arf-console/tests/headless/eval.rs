use super::support::*;
use std::time::Duration;

#[test]
fn test_headless_starts_and_ipc_ready() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process.ipc_session().expect("ipc session should run");
    assert!(
        result.success,
        "ipc session should succeed. stdout: {}, stderr: {}",
        result.stdout, result.stderr
    );
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["pid"].as_u64(),
        Some(process.pid as u64),
        "session should show correct PID: {}",
        result.stdout
    );
}

/// Test that `arf ipc eval` returns a visible R value.
#[test]
fn test_headless_eval_value() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process.ipc_eval("1 + 1").expect("eval should run");
    assert!(
        result.success,
        "eval should succeed. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("[1] 2"),
        "should capture R value: {}",
        result.stdout
    );
}

/// Test that `arf ipc eval` captures stdout from `cat()`.
#[test]
fn test_headless_eval_stdout() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval("cat('hello_headless\\n')")
        .expect("eval should run");
    assert!(
        result.success,
        "eval should succeed. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("hello_headless"),
        "should capture stdout: {}",
        result.stdout
    );
}

/// Test that `arf ipc eval` reports R errors in the JSON response.
#[test]
fn test_headless_eval_error() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval("stop('headless_error')")
        .expect("eval should run");
    // R errors are returned as part of the JSON response (exit 0)
    assert!(
        result.success,
        "eval should succeed (R errors are in JSON, not exit code). stderr: {}",
        result.stderr
    );
    let json = parse_ipc_json(&result);
    assert!(
        json["error"]
            .as_str()
            .is_some_and(|s| s.contains("headless_error")),
        "should report error in JSON: {}",
        result.stdout
    );
}

/// Test sequential evaluations: state persists across calls.
#[test]
fn test_headless_eval_sequential() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // Assign a variable
    let r1 = process.ipc_eval("x <- 42").expect("first eval should run");
    assert!(r1.success, "first eval should succeed");

    // Use the variable
    let r2 = process.ipc_eval("x * 2").expect("second eval should run");
    assert!(r2.success, "second eval should succeed");
    assert!(
        r2.stdout.contains("[1] 84"),
        "should see variable from first eval: {}",
        r2.stdout
    );
}

/// Test that `arf ipc eval` captures both stdout and value.
#[test]
fn test_headless_eval_mixed_output() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval("cat('before\\n'); 42")
        .expect("eval should run");
    assert!(result.success, "eval should succeed");
    assert!(
        result.stdout.contains("before"),
        "should capture stdout: {}",
        result.stdout
    );
    assert!(
        result.stdout.contains("[1] 42"),
        "should capture value: {}",
        result.stdout
    );
}

/// Test that `arf ipc send` (user_input) is accepted in headless mode.
#[test]
fn test_headless_user_input() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_send("invisible(NULL)")
        .expect("send should run");
    assert!(
        result.success,
        "send should succeed. stderr: {}",
        result.stderr
    );
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["accepted"].as_bool(),
        Some(true),
        "should report acceptance: {}",
        result.stdout
    );
}

/// Test multiline R code evaluation in headless mode.
#[test]
fn test_headless_eval_multiline() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let code = r#"f <- function(x) x + 1; f(10)"#;
    let result = process.ipc_eval(code).expect("eval should run");
    assert!(result.success, "eval should succeed");
    assert!(
        result.stdout.contains("[1] 11"),
        "should evaluate multiline code: {}",
        result.stdout
    );
}

/// Test that `arf ipc eval --visible` outputs to the headless process's stderr.
///
/// When `--visible` is used, the evaluated output should appear both in the
/// JSON-RPC response AND on the headless process's stdout/stderr (via
/// WriteConsoleEx passthrough). This is useful for monitoring/logging.
#[test]
fn test_headless_eval_visible() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // Use a unique marker to avoid matching startup messages
    let result = process
        .ipc_eval_visible("cat('vis_marker_42\\n')")
        .expect("visible eval should run");
    assert!(
        result.success,
        "visible eval should succeed. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("vis_marker_42"),
        "JSON-RPC response should capture stdout: {}",
        result.stdout
    );

    // Give the headless process a moment to flush output
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Verify the output also appeared on the headless process's stdout/stderr.
    // R's cat() goes through WriteConsoleEx non-error channel → print! → stdout.
    let server_output = process.server_output();
    assert!(
        server_output.contains("vis_marker_42"),
        "visible eval output should appear on headless process output: {}",
        server_output
    );
}

/// Test that silent eval does NOT output to the headless process.
#[test]
fn test_headless_eval_silent_no_server_output() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval("cat('silent_marker_99\\n')")
        .expect("eval should run");
    assert!(result.success, "eval should succeed");
    assert!(
        result.stdout.contains("silent_marker_99"),
        "JSON-RPC response should capture stdout: {}",
        result.stdout
    );

    // Give a moment for any output to flush
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Silent eval should NOT appear on the headless process
    let server_output = process.server_output();
    assert!(
        !server_output.contains("silent_marker_99"),
        "silent eval output should NOT appear on headless process output: {}",
        server_output
    );
}

/// Test that `--vanilla` flag works in headless mode.
#[test]
fn test_headless_vanilla_flag() {
    let process =
        HeadlessProcess::spawn_with_args(&["--vanilla"]).expect("Failed to spawn with --vanilla");

    let result = process.ipc_eval("1 + 1").expect("eval should run");
    assert!(
        result.success,
        "eval should succeed with --vanilla. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("[1] 2"),
        "should return result: {}",
        result.stdout
    );
}

/// Test that --timeout option works: a fast eval completes within timeout.
#[test]
fn test_headless_eval_timeout_sufficient() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // Fast expression with generous timeout should succeed
    let result = process
        .ipc_eval_with_timeout("1 + 1", 30000)
        .expect("eval with timeout should run");
    assert!(result.success, "should succeed: {}", result.stderr);
    assert!(
        result.stdout.contains("[1] 2"),
        "should return result: {}",
        result.stdout
    );
}

/// Test that --timeout option works: a slow eval times out.
#[test]
fn test_headless_eval_timeout_exceeded() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // Sys.sleep(10) with a 1-second timeout should fail
    let result = process
        .ipc_eval_with_timeout("Sys.sleep(10)", 1000)
        .expect("eval with timeout should run");
    assert!(
        !result.success,
        "should fail due to timeout. stdout: {}, stderr: {}",
        result.stdout, result.stderr
    );
    assert!(
        result.stderr.contains("timed out"),
        "should mention timeout: {}",
        result.stderr
    );
}

/// Test that `arf ipc shutdown` gracefully stops a headless process.
#[test]
fn test_headless_shutdown_via_ipc() {
    let mut process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // Verify it's running
    let session = process.ipc_session().expect("session should work");
    assert!(session.success, "should be running");

    // Send shutdown
    let result = process.ipc_shutdown().expect("shutdown should run");
    assert!(
        result.success,
        "shutdown should succeed. stderr: {}",
        result.stderr
    );
    let json = parse_ipc_json(&result);
    assert_eq!(
        json["accepted"].as_bool(),
        Some(true),
        "should report acceptance: {}",
        result.stdout
    );

    // Process should exit within a few seconds
    process
        .wait_for_exit(Duration::from_secs(10))
        .expect("headless process should exit after shutdown");
}

/// Test that help pages are captured via the custom pager instead of
/// spawning an interactive pager like `less`.
#[test]
fn test_headless_help_does_not_hang() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // ?mean triggers R's help system which would normally open a pager.
    // With our custom pager, the help text should be captured in stdout.
    let result = process
        .ipc_eval_with_timeout("?mean", 15000)
        .expect("help eval should run");
    assert!(
        result.success,
        "help should succeed without hanging. stderr: {}",
        result.stderr
    );
    // The help text for `mean` should contain the word "mean" somewhere
    assert!(
        result.stdout.to_lowercase().contains("mean"),
        "help output should contain 'mean': {}",
        result.stdout
    );
}

/// Test that plot() does not hang or error in headless mode.
/// The graphics device should default to a file-based device (png/pdf).
#[test]
fn test_headless_plot_does_not_hang() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    // plot() would normally try to open X11/quartz. In headless mode,
    // our custom device function should create a file-based device instead.
    let result = process
        .ipc_eval_with_timeout(
            "plot(1:10); dev_name <- names(dev.cur()); dev.off(); cat(dev_name)",
            15000,
        )
        .expect("plot eval should run");
    assert!(
        result.success,
        "plot should succeed without hanging. stderr: {}",
        result.stderr
    );
    // Verify the device is non-interactive: png/pdf from our custom device,
    // or quartz_off_screen on macOS (quartz works headlessly unlike X11)
    let stdout = &result.stdout;
    assert!(
        stdout.contains("png") || stdout.contains("pdf") || stdout.contains("quartz_off_screen"),
        "graphics device should be non-interactive, got: {}",
        stdout
    );
}

/// Test that browseURL() prints the URL to stdout instead of opening a browser.
#[test]
fn test_headless_browse_url_does_not_hang() {
    let process = HeadlessProcess::spawn().expect("Failed to spawn headless");

    let result = process
        .ipc_eval_with_timeout("browseURL('https://example.com')", 15000)
        .expect("browseURL eval should run");
    assert!(
        result.success,
        "browseURL should succeed without hanging. stderr: {}",
        result.stderr
    );
    assert!(
        result.stdout.contains("https://example.com"),
        "URL should be captured in stdout: {}",
        result.stdout
    );
}
