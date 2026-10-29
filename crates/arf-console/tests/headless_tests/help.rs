use super::support::{HeadlessProcess, run_ipc_command};
use serde_json::Value;
use std::time::{Duration, Instant};

fn help_request(pid: u32, args: &[&str]) -> Value {
    let pid = pid.to_string();
    let mut command = vec!["ipc", "r-help"];
    command.extend_from_slice(args);
    command.extend(["--pid", &pid]);
    let output = run_ipc_command(&command);
    assert!(
        output.status.success(),
        "help request failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("help JSON")
}

#[test]
fn r_help_search_and_show_respond_while_r_is_busy() {
    let temp = tempfile::tempdir().unwrap();
    let process =
        HeadlessProcess::spawn_with_args(&["--vanilla", "--no-history"]).expect("headless process");
    let initial = help_request(process.pid, &["show", "sum", "--package", "base"]);
    assert_eq!(initial["kind"], "page");
    assert_eq!(initial["content_type"], "markdown");
    assert!(initial["content"].as_str().unwrap().contains("Sum"));
    assert!(!initial["library_paths"].as_array().unwrap().is_empty());

    let started = temp.path().join("started");
    let release = temp.path().join("release");
    let started_r = serde_json::to_string(started.to_str().unwrap()).unwrap();
    let release_r = serde_json::to_string(release.to_str().unwrap()).unwrap();
    let code = format!(
        r#"file.create({started_r}); local({{ deadline <- Sys.time() + 30; while (!file.exists({release_r}) && Sys.time() < deadline) Sys.sleep(0.01); if (!file.exists({release_r})) stop("help test gate timed out") }})"#
    );
    assert!(process.ipc_send(&code).unwrap().success);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !started.exists() {
        assert!(Instant::now() < deadline, "R did not start gate");
        std::thread::sleep(Duration::from_millis(25));
    }

    let pid = process.pid;
    let (sender, receiver) = std::sync::mpsc::channel();
    let requests = std::thread::spawn(move || {
        let page = help_request(pid, &["show", "sum", "--package", "base"]);
        let topics = help_request(pid, &["search", "--package", "base", "--query", "sum"]);
        let missing = run_ipc_command(&[
            "ipc",
            "r-help",
            "show",
            "arf_nonexistent_help_topic",
            "--package",
            "base",
            "--pid",
            &pid.to_string(),
        ]);
        assert!(!missing.status.success());
        let error: Value = serde_json::from_slice(&missing.stderr).expect("help error JSON");
        sender.send((page, topics, error)).unwrap();
    });
    let busy_results = receiver.recv_timeout(Duration::from_secs(10));
    // Release R even if an IPC request incorrectly waited for it.
    std::fs::write(&release, []).unwrap();
    let (page, topics, error) =
        busy_results.expect("both help methods must respond while R is blocked");
    requests.join().unwrap();
    assert_eq!(page["content"], initial["content"]);
    for result in [&page, &topics] {
        assert_eq!(result["library_paths"], initial["library_paths"]);
    }
    assert!(!topics["topics"].as_array().unwrap().is_empty());
    assert_eq!(error["error"]["code"], "HELP_NOT_FOUND");
}
