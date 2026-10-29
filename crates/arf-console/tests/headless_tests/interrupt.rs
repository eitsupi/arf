use super::support::{HeadlessProcess, parse_ipc_json, run_ipc_command};
use std::path::Path;
use std::thread;
use std::time::Duration;

fn interrupt_process(pid: u32, execution_id: Option<u64>) -> serde_json::Value {
    let pid = pid.to_string();
    let execution_id = execution_id.map(|id| id.to_string());
    let mut args = vec!["ipc", "interrupt", "--pid", pid.as_str()];
    if let Some(id) = execution_id.as_deref() {
        args.extend(["--execution-id", id]);
    }
    let output = run_ipc_command(&args);
    assert!(
        output.status.success(),
        "interrupt command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("interrupt response should be JSON")
}

fn eval_process(pid: u32, code: &str, visible: bool) -> serde_json::Value {
    let pid = pid.to_string();
    let mut args = vec![
        "ipc",
        "eval",
        code,
        "--pid",
        pid.as_str(),
        "--timeout",
        "30000",
    ];
    if visible {
        args.push("--visible");
    }
    let output = run_ipc_command(&args);
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "eval response should be JSON: {error}; status={}; stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn wait_for_file(path: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "evaluation marker was not created: {}",
            path.display()
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
fn ipc_interrupt_stops_evaluations_and_keeps_the_headless_session_alive() {
    let process = HeadlessProcess::spawn().expect("headless process should start");

    let idle = interrupt_process(process.pid, None);
    assert_eq!(idle["accepted"], false, "idle interrupt: {idle}");

    for visible in [false, true] {
        let marker_dir = tempfile::tempdir().expect("marker directory");
        let marker = marker_dir.path().join("evaluation-started");
        let marker_r = serde_json::to_string(&marker.to_string_lossy().as_ref())
            .expect("R marker path string");
        let code = format!("interrupt_value <- 42L; file.create({marker_r})\nSys.sleep(30)");
        let pid = process.pid;
        let evaluation = thread::spawn(move || eval_process(pid, &code, visible));
        wait_for_file(&marker);
        let accepted = interrupt_process(process.pid, None);
        assert_eq!(
            accepted["accepted"], true,
            "interrupt should be accepted: {accepted}"
        );
        let result = evaluation
            .join()
            .expect("evaluation client thread should finish");
        assert!(
            result["error"]
                .as_str()
                .is_some_and(|error| error.to_ascii_lowercase().contains("interrupt")),
            "evaluation should report interruption: {result}"
        );
        assert_eq!(result["status"], "interrupted", "{result}");
        let value = process
            .ipc_eval("interrupt_value")
            .expect("session should accept a follow-up evaluation");
        assert!(value.success, "session did not recover: {}", value.stderr);
        assert_eq!(parse_ipc_json(&value)["value"], "[1] 42");
    }

    let marker_dir = tempfile::tempdir().expect("send marker directory");
    let marker = marker_dir.path().join("send-started");
    let marker_r =
        serde_json::to_string(&marker.to_string_lossy().as_ref()).expect("R marker path string");
    let send_code = format!(
        "interrupt_send_value <- 17L; file.create({marker_r}); Sys.sleep(30)\ninterrupt_remainder <- 99L"
    );
    let send = process
        .ipc_send(&send_code)
        .expect("send should be accepted");
    assert!(send.success, "send should be accepted: {}", send.stderr);
    let send_response = parse_ipc_json(&send);
    let execution_id = send_response["execution_id"]
        .as_u64()
        .expect("accepted send should return an execution ID");
    wait_for_file(&marker);
    let accepted = interrupt_process(process.pid, Some(execution_id));
    assert_eq!(accepted["accepted"], true, "send interrupt: {accepted}");
    let follow_up = process
        .ipc_eval("interrupt_send_value")
        .expect("session should survive an interrupted send");
    assert!(follow_up.success, "follow-up failed: {}", follow_up.stderr);
    assert_eq!(parse_ipc_json(&follow_up)["value"], "[1] 17");
    let remainder = process
        .ipc_eval(r#"exists('interrupt_remainder', envir = .GlobalEnv)"#)
        .expect("inspect multiline remainder");
    assert_eq!(parse_ipc_json(&remainder)["value"], "[1] FALSE");
}

#[test]
fn idle_and_stale_interrupts_do_not_affect_later_headless_evaluations() {
    let process = HeadlessProcess::spawn().expect("headless process should start");
    let stale = interrupt_process(process.pid, Some(u64::MAX));
    assert_eq!(
        stale["accepted"], false,
        "unknown ID should be rejected: {stale}"
    );

    // A stale ID must not stop a later evaluation that is running.
    let marker_dir = tempfile::tempdir().expect("marker directory");
    let marker = marker_dir.path().join("stale-started");
    let marker_r =
        serde_json::to_string(&marker.to_string_lossy().as_ref()).expect("R marker path string");
    let code = format!("file.create({marker_r}); Sys.sleep(1); stale_survivor <- 42L");
    let pid = process.pid;
    let evaluation = thread::spawn(move || eval_process(pid, &code, true));
    wait_for_file(&marker);
    let stale = interrupt_process(process.pid, Some(u64::MAX));
    assert_eq!(
        stale["accepted"], false,
        "stale ID must be rejected while another evaluation runs: {stale}"
    );
    let finished = evaluation.join().expect("evaluation client thread");
    assert!(
        finished["error"].is_null(),
        "stale interrupt stopped a later evaluation: {finished}"
    );
    let survivor = process
        .ipc_eval("stale_survivor")
        .expect("variable assigned after the sleep should exist");
    assert_eq!(parse_ipc_json(&survivor)["value"], "[1] 42");

    let error = process
        .ipc_eval("stop('ordinary interrupt regression')")
        .expect("ordinary error response should arrive");
    assert!(error.success, "evaluate transport failed: {}", error.stderr);
    let error_json = parse_ipc_json(&error);
    assert!(
        error_json["error"]
            .as_str()
            .is_some_and(|message| message.contains("ordinary interrupt regression")),
        "ordinary R error was not preserved: {error_json}"
    );
    let next = process
        .ipc_eval("6 * 7")
        .expect("evaluation after stale interrupt should succeed");
    assert_eq!(parse_ipc_json(&next)["value"], "[1] 42");
}

#[cfg(unix)]
#[test]
fn headless_interrupted_evaluate_status_matches_completion() {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    let process = HeadlessProcess::spawn().expect("headless process should start");
    let session = process.ipc_session().expect("IPC session should run");
    let socket_path = parse_ipc_json(&session)["socket_path"]
        .as_str()
        .expect("session socket path")
        .to_owned();
    let stream = UnixStream::connect(socket_path).expect("connect completion subscriber");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("set subscriber timeout");
    let mut reader = BufReader::new(stream);
    writeln!(
        reader.get_mut(),
        r#"{{"jsonrpc":"2.0","id":1,"method":"attach","params":{{"subscribe":["arf.execution_completed"]}}}}"#
    )
    .expect("send attach request");
    reader.get_mut().flush().expect("flush attach request");
    let mut attached = String::new();
    reader
        .read_line(&mut attached)
        .expect("read attach response");
    let attached: serde_json::Value = serde_json::from_str(&attached).expect("attach JSON");
    assert_eq!(attached["id"], 1, "{attached}");

    for visible in [false, true] {
        let marker_dir = tempfile::tempdir().expect("marker directory");
        let marker = marker_dir.path().join("evaluation-started");
        let marker_r = serde_json::to_string(&marker.to_string_lossy().as_ref())
            .expect("R marker path string");
        let code = format!("file.create({marker_r}); Sys.sleep(30)");
        let pid = process.pid;
        let evaluation = thread::spawn(move || eval_process(pid, &code, visible));
        wait_for_file(&marker);
        assert_eq!(interrupt_process(process.pid, None)["accepted"], true);
        let response = evaluation.join().expect("evaluation thread");
        assert_eq!(response["status"], "interrupted", "{response}");
        let execution_id = response["execution_id"].as_u64().expect("execution ID");

        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("read completion event");
            let event: serde_json::Value = serde_json::from_str(&line).expect("completion JSON");
            if event["params"]["topic"] == "arf.execution_completed"
                && event["params"]["data"]["id"] == execution_id
            {
                assert_eq!(event["params"]["data"]["status"], "interrupted", "{event}");
                break;
            }
        }
    }
}
