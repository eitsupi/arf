use super::support::{HeadlessProcess, parse_ipc_json};
use std::time::{Duration, Instant};

fn callback_log(process: &HeadlessProcess) -> String {
    let result = process
        .ipc_eval(
            r#"cat(paste(c(.arf_e2e_log, paste0("COUNT=", length(.arf_e2e_log))), collapse="\n"))"#,
        )
        .expect("inspect callback log");
    assert!(result.success, "log inspection failed: {}", result.stderr);
    parse_ipc_json(&result)["stdout"]
        .as_str()
        .expect("callback log inspection should return stdout")
        .to_owned()
}

fn assert_incomplete_request(result: &super::support::IpcOutput) {
    assert_eq!(result.exit_code, Some(4), "{}", result.stderr);
    let response: serde_json::Value =
        serde_json::from_str(&result.stderr).unwrap_or_else(|error| {
            panic!("expected structured IPC error: {error}: {}", result.stderr)
        });
    assert_eq!(response["error"]["code"], "INCOMPLETE_INPUT", "{response}");
    assert_eq!(
        response["error"]["message"], "R code is syntactically incomplete",
        "{response}"
    );
}

fn wait_for_callback_count(process: &HeadlessProcess, count: usize) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let log = callback_log(process);
        if log.lines().any(|line| line == format!("COUNT={count}")) {
            return log;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {count} callbacks; latest log: {log:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_callback_record(process: &HeadlessProcess, prefix: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let log = callback_log(process);
        if log.lines().any(|line| line.starts_with(prefix)) {
            return log;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for callback {prefix:?}; latest log: {log:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn install_task_callback(process: &HeadlessProcess) {
    let result = process
        .ipc_eval(
            r#".arf_e2e_log <- character(); addTaskCallback(function(expr, value, ok, visible) { record <- paste(paste(deparse(expr), collapse=" "), paste(deparse(value), collapse=" "), ok, visible, sep="|"); .arf_e2e_log <<- c(.arf_e2e_log, record); TRUE }, name="headless-e2e")"#,
        )
        .expect("install task callback");
    assert!(result.success, "callback setup failed: {}", result.stderr);
    assert!(parse_ipc_json(&result)["error"].is_null());

    // Setup and reset are silent operations; start with an empty callback log.
    let result = process
        .ipc_eval(".arf_e2e_log <- character()")
        .expect("reset callback log");
    assert!(
        result.success,
        "callback log reset failed: {}",
        result.stderr
    );
}

#[test]
fn headless_send_and_visible_eval_use_repl_top_level_callbacks() {
    let process = HeadlessProcess::spawn().expect("spawn headless");
    install_task_callback(&process);

    let sent = process
        .ipc_send("10\ninvisible(20)\n{ 30; 40 }")
        .expect("send top-level expressions");
    assert!(sent.success, "send failed: {}", sent.stderr);
    assert_eq!(parse_ipc_json(&sent)["accepted"], true);

    let log = wait_for_callback_count(&process, 3);
    let entries = log.lines().take(3).collect::<Vec<_>>();
    assert!(entries[0].starts_with("10|10|TRUE|TRUE"), "{log}");
    assert!(entries[1].contains("invisible(20)|20|TRUE|FALSE"), "{log}");
    assert!(
        entries[2].contains("30")
            && entries[2].contains("40")
            && entries[2].ends_with("|40|TRUE|TRUE"),
        "a braced block should produce one callback containing its final value: {log}"
    );

    // Silent evaluation returns a value but must not add a task callback.
    let silent = process.ipc_eval("99").expect("silent evaluation");
    assert!(
        silent.success,
        "silent evaluation failed: {}",
        silent.stderr
    );
    let log = callback_log(&process);
    assert!(log.lines().any(|line| line == "COUNT=3"), "{log}");
    let last_value = process
        .ipc_eval("isTRUE(.Last.value == 40)")
        .expect("inspect .Last.value after silent evaluation");
    assert!(
        last_value.success,
        "inspection failed: {}",
        last_value.stderr
    );
    assert_eq!(parse_ipc_json(&last_value)["value"], "[1] TRUE");

    let visible = process
        .ipc_eval_visible("51; invisible(52); 53")
        .expect("visible evaluation");
    assert!(
        visible.success,
        "visible evaluation failed: {}",
        visible.stderr
    );
    let visible_json = parse_ipc_json(&visible);
    assert!(visible_json["value"].is_null(), "{visible_json}");
    assert!(visible_json["error"].is_null(), "{visible_json}");
    let visible_stdout = visible_json["stdout"].as_str().expect("visible stdout");
    assert!(visible_stdout.contains("[1] 51"), "{visible_json}");
    assert!(visible_stdout.contains("[1] 53"), "{visible_json}");
    let log = wait_for_callback_count(&process, 6);
    assert!(
        log.lines().any(|line| line.starts_with("51|51|TRUE|TRUE")),
        "first visible callback has incorrect arguments: {log}"
    );
    assert!(
        log.lines()
            .any(|line| line.contains("invisible(52)|52|TRUE|FALSE")),
        "invisible visible-eval callback has incorrect arguments: {log}"
    );
    assert!(
        log.lines().any(|line| line.starts_with("53|53|TRUE|TRUE")),
        "last visible callback has incorrect arguments: {log}"
    );

    let last_value = process
        .ipc_eval("isTRUE(.Last.value == 53)")
        .expect("inspect .Last.value");
    assert!(
        last_value.success,
        "inspection failed: {}",
        last_value.stderr
    );
    assert_eq!(parse_ipc_json(&last_value)["value"], "[1] TRUE");
}

#[test]
fn headless_long_utf8_request_survives_r_console_buffer_boundary() {
    let process = HeadlessProcess::spawn().expect("spawn headless");
    let payload = "λ界🙂".repeat(1_300);
    let code = format!(r#"cat("{payload}")"#);
    assert!(code.len() > 4_096);

    let result = process.ipc_eval_visible(&code).expect("visible long eval");
    assert!(result.success, "long eval failed: {}", result.stderr);
    let response = parse_ipc_json(&result);
    let execution_id = response["execution_id"].as_u64().expect("execution ID");
    assert_eq!(
        response,
        serde_json::json!({
            "execution_id": execution_id,
            "stdout": payload,
            "stderr": "",
            "value": null,
            "error": null,
        })
    );
}

#[test]
fn headless_visible_long_comment_and_whitespace_prefixes_preserve_next_expression() {
    let process = HeadlessProcess::spawn().expect("spawn headless");
    install_task_callback(&process);

    let spaces = " ".repeat(5_000);
    let cases: [(&str, String, &[&str]); 5] = [
        (
            "comment prefix",
            format!("# {}\n77", "x".repeat(5_000)),
            &["77"],
        ),
        ("whitespace prefix", format!("{spaces}77"), &["77"]),
        (
            "between expressions",
            format!("78;{spaces}79"),
            &["78", "79"],
        ),
        (
            "string content",
            format!(r#"nchar("{spaces}  ")"#),
            &["5002L"],
        ),
        ("function call", format!("identity{spaces}(80)"), &["80"]),
    ];
    let mut callback_count = 0;
    for (case, code, values) in cases {
        let result = process
            .ipc_eval_visible(&code)
            .unwrap_or_else(|error| panic!("{case} eval should run: {error}"));
        assert!(result.success, "{case} eval failed: {}", result.stderr);
        let response = parse_ipc_json(&result);
        let stdout = response["stdout"].as_str().expect("visible stdout");
        for value in values {
            let printed = format!("[1] {}", value.trim_end_matches('L'));
            assert!(
                stdout.contains(&printed),
                "{case} missed {printed}: {response}"
            );
        }

        let previous_count = callback_count;
        callback_count += values.len();
        let log = wait_for_callback_count(&process, callback_count);
        for (entry, value) in log.lines().skip(previous_count).zip(values) {
            assert!(
                entry.ends_with(&format!("|{value}|TRUE|TRUE")),
                "{case} callback has incorrect value or visibility: {log}"
            );
        }
    }
    assert!(
        process
            .ipc_session()
            .expect("session should remain alive")
            .success
    );
}

#[test]
fn headless_error_discards_long_unconsumed_request_remainder() {
    let process = HeadlessProcess::spawn().expect("spawn headless");
    let padding = "x".repeat(5_000);
    let code = format!("stop('HEADLESS_EARLY_ERROR')\n# {padding}\nheadless_after_error <- TRUE");
    assert!(code.len() > 4_096);

    let failed = process.ipc_send(&code).expect("send long failing request");
    assert!(failed.success, "send failed: {}", failed.stderr);
    assert_eq!(parse_ipc_json(&failed)["accepted"], true);

    let inspect = process
        .ipc_eval("exists('headless_after_error', envir=.GlobalEnv, inherits=FALSE)")
        .expect("inspect discarded request remainder");
    assert!(inspect.success, "inspection failed: {}", inspect.stderr);
    assert_eq!(parse_ipc_json(&inspect)["value"], "[1] FALSE");
}

fn wait_for_server_output(process: &HeadlessProcess, marker: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = process.server_output();
        if output.contains(marker) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {marker:?} in headless output: {output}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn nested_readline_and_browser_return_eof_without_consuming_next_request() {
    let process = HeadlessProcess::spawn().expect("spawn headless");
    let setup = process
        .ipc_eval("options(prompt='HEADLESS_NESTED_PROMPT> ')")
        .expect("configure nested prompt");
    assert!(setup.success, "prompt setup failed: {}", setup.stderr);

    let readline = process
        .ipc_send(concat!(
            "cat('HEADLESS_READLINE_WINDOW\\n'); Sys.sleep(1); ",
            "headless_readline_result <- readline(options('prompt')[[1]])"
        ))
        .expect("send nested readline");
    assert!(
        readline.success,
        "readline send failed: {}",
        readline.stderr
    );
    assert_eq!(parse_ipc_json(&readline)["accepted"], true);
    wait_for_server_output(&process, "HEADLESS_READLINE_WINDOW");

    let readline_followup = process
        .ipc_send("headless_readline_followup_frame <- sys.nframe()")
        .expect("queue request during nested readline");
    assert!(
        readline_followup.success,
        "readline follow-up send failed: {}",
        readline_followup.stderr
    );
    assert_eq!(parse_ipc_json(&readline_followup)["accepted"], true);
    let readline_state = process
        .ipc_eval(
            "isTRUE(identical(headless_readline_result, '') && headless_readline_followup_frame == 0)",
        )
        .expect("inspect readline EOF and follow-up");
    assert!(
        readline_state.success,
        "inspection failed: {}",
        readline_state.stderr
    );
    assert_eq!(parse_ipc_json(&readline_state)["value"], "[1] TRUE");

    let browser = process
        .ipc_send("cat('HEADLESS_BROWSER_WINDOW\\n'); Sys.sleep(1); browser()")
        .expect("send top-level browser");
    assert!(browser.success, "browser send failed: {}", browser.stderr);
    assert_eq!(parse_ipc_json(&browser)["accepted"], true);
    wait_for_server_output(&process, "HEADLESS_BROWSER_WINDOW");

    let browser_followup = process
        .ipc_send("headless_browser_followup_frame <- sys.nframe()")
        .expect("queue request during browser prompt");
    assert!(
        browser_followup.success,
        "browser follow-up send failed: {}",
        browser_followup.stderr
    );
    assert_eq!(parse_ipc_json(&browser_followup)["accepted"], true);
    let browser_state = process
        .ipc_eval("isTRUE(headless_browser_followup_frame == 0)")
        .expect("inspect browser follow-up");
    assert!(
        browser_state.success,
        "inspection failed: {}",
        browser_state.stderr
    );
    assert_eq!(parse_ipc_json(&browser_state)["value"], "[1] TRUE");
}

#[test]
fn nested_parse_returns_eof_and_preserves_outer_eval_and_followup_request() {
    let process = HeadlessProcess::spawn().expect("spawn headless");
    let code = concat!(
        "cat('HEADLESS_PARSE_WINDOW\\n'); Sys.sleep(1); ",
        "headless_parse_result <- parse(n=1, prompt=getOption('prompt')); ",
        "cat('HEADLESS_PARSE_OUTER_DONE\\n')"
    );
    let pid = process.pid.to_string();
    let visible_eval = std::thread::spawn(move || {
        super::support::run_ipc_command(&["ipc", "eval", code, "--pid", &pid, "--visible"])
    });

    wait_for_server_output(&process, "HEADLESS_PARSE_WINDOW");
    let followup = process
        .ipc_send("headless_parse_followup_frame <- sys.nframe()")
        .expect("queue request during nested parse");
    assert!(
        followup.success,
        "follow-up send failed: {}",
        followup.stderr
    );
    assert_eq!(parse_ipc_json(&followup)["accepted"], true);

    let visible_output = visible_eval
        .join()
        .expect("visible eval client thread should finish");
    assert!(
        visible_output.status.success(),
        "visible eval failed: {}",
        String::from_utf8_lossy(&visible_output.stderr)
    );
    let visible_json: serde_json::Value =
        serde_json::from_slice(&visible_output.stdout).expect("visible eval should return JSON");
    assert!(
        visible_json["stdout"]
            .as_str()
            .is_some_and(|stdout| stdout.contains("HEADLESS_PARSE_OUTER_DONE")),
        "outer expression did not continue after parse EOF: {visible_json}"
    );

    let state = process
        .ipc_eval(
            "isTRUE(length(headless_parse_result) == 0 && headless_parse_followup_frame == 0)",
        )
        .expect("inspect parse EOF and top-level follow-up");
    assert!(state.success, "inspection failed: {}", state.stderr);
    assert_eq!(parse_ipc_json(&state)["value"], "[1] TRUE");
}

#[test]
fn headless_error_and_incomplete_eval_leave_the_next_request_usable() {
    let process = HeadlessProcess::spawn().expect("spawn headless");
    install_task_callback(&process);

    let lexical_error = process
        .ipc_eval_visible(r#"headless_lexical_prefix <- 7; '\q'"#)
        .expect("visible eval with a lexical error after a valid expression");
    assert!(
        lexical_error.success,
        "visible lexical error should be reported by R: {}",
        lexical_error.stderr
    );
    let lexical_json = parse_ipc_json(&lexical_error);
    assert_eq!(
        lexical_json["error"],
        serde_json::Value::Null,
        "{lexical_json}"
    );
    assert_eq!(
        lexical_json["value"],
        serde_json::Value::Null,
        "{lexical_json}"
    );
    assert!(
        lexical_json["stderr"]
            .as_str()
            .is_some_and(|stderr| !stderr.is_empty()),
        "R should report the lexical error on stderr: {lexical_json}"
    );

    let prefix_state = process
        .ipc_eval("isTRUE(identical(headless_lexical_prefix, 7))")
        .expect("inspect expression before lexical error");
    assert!(
        prefix_state.success,
        "inspection failed: {}",
        prefix_state.stderr
    );
    assert_eq!(parse_ipc_json(&prefix_state)["value"], "[1] TRUE");
    let log = callback_log(&process);
    assert!(
        log.lines()
            .any(|line| line.contains("headless_lexical_prefix <- 7|7|TRUE|FALSE")),
        "valid prefix should have its own invisible assignment callback: {log}"
    );

    let failed = process
        .ipc_send("stop('HEADLESS_TOP_LEVEL_ERROR')")
        .expect("send failing expression");
    assert!(failed.success, "send failed: {}", failed.stderr);
    assert_eq!(parse_ipc_json(&failed)["accepted"], true);

    // A silent inspection is also a main-loop boundary after the runtime error.
    let _ = callback_log(&process);

    let incomplete_send = process
        .ipc_send("function(x) {")
        .expect("submit incomplete send");
    assert_incomplete_request(&incomplete_send);
    let incomplete_visible = process
        .ipc_eval_visible("function(x) {")
        .expect("submit incomplete visible eval");
    assert_incomplete_request(&incomplete_visible);

    // A complete request with a valid prefix and a syntax error in the middle
    // should run the prefix, report the error, and leave the next request usable.
    let syntax_error = process
        .ipc_send("8\n)\n9")
        .expect("send valid prefix followed by a syntax error");
    assert!(syntax_error.success, "send failed: {}", syntax_error.stderr);
    assert_eq!(parse_ipc_json(&syntax_error)["accepted"], true);
    let log = callback_log(&process);
    assert!(
        log.lines().any(|line| line.starts_with("8|8|TRUE|TRUE")),
        "{log}"
    );

    let next = process
        .ipc_send("7")
        .expect("send after error and incomplete input");
    assert!(next.success, "recovery send failed: {}", next.stderr);
    assert_eq!(parse_ipc_json(&next)["accepted"], true);
    let log = wait_for_callback_record(&process, "7|7|TRUE|TRUE");
    assert!(
        log.lines().any(|line| line.starts_with("8|8|TRUE|TRUE")),
        "{log}"
    );

    let last_value = process
        .ipc_eval("isTRUE(.Last.value == 7)")
        .expect("inspect recovered .Last.value");
    assert!(
        last_value.success,
        "inspection failed: {}",
        last_value.stderr
    );
    assert_eq!(parse_ipc_json(&last_value)["value"], "[1] TRUE");
}
