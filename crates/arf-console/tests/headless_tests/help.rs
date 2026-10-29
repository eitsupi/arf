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
fn r_help_search_candidates_can_be_selected_and_operator_links_followed() {
    let library = tempfile::tempdir().unwrap();
    let package = library.path().join("resolverfixture");
    std::fs::create_dir_all(package.join("help")).unwrap();
    std::fs::create_dir_all(package.join("Meta")).unwrap();
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../arf-harp/tests/fixtures/help_metadata");
    for extension in ["rdx", "rdb"] {
        std::fs::copy(
            fixtures.join(format!("../help_resolution/resolverpkg.{extension}")),
            package.join(format!("help/resolverfixture.{extension}")),
        )
        .unwrap();
    }
    std::fs::copy(
        fixtures.join("help_topics_metadata_v3.rds"),
        package.join("Meta/Rd.rds"),
    )
    .unwrap();
    std::fs::copy(
        fixtures.join(r"../help_resolution/aliases.rds"),
        package.join(r"help/aliases.rds"),
    )
    .unwrap();
    std::fs::write(package.join("Meta/package.rds"), []).unwrap();
    let process = HeadlessProcess::spawn_with_args_and_env(
        &["--vanilla", "--no-history"],
        &[("R_LIBS", library.path().to_str().unwrap())],
    )
    .expect("headless process");
    let search = help_request(
        process.pid,
        &[
            "search",
            "--query",
            "shared",
            "--package",
            "resolverfixture",
        ],
    );
    let topic = search["topics"][0]["aliases"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|alias| alias.as_str().filter(|alias| *alias == "shared"))
        .expect("search returns the shared alias");
    let first = &search[r"topics"][0];
    let selected = help_request(
        process.pid,
        &[
            r"show",
            first[r"topic"].as_str().unwrap(),
            r"--package",
            first[r"package"].as_str().unwrap(),
            r"--help-key",
            first[r"help_key"].as_str().unwrap(),
        ],
    );
    assert_eq!(selected[r"kind"], r"page");
    assert_eq!(selected[r"title"], first[r"title"]);
    let explicit = help_request(
        process.pid,
        &[r"show", topic, r"--package", r"resolverfixture"],
    );
    assert_eq!(explicit[r"kind"], r"page");
    assert!(
        explicit[r"content"]
            .as_str()
            .unwrap()
            .contains(r"Fixture first-topic")
    );
    let result = help_request(process.pid, &[r"show", topic]);
    assert_eq!(result["kind"], "candidates");
    let candidates = result["candidates"].as_array().unwrap();
    assert!(candidates.len() >= 2);
    for candidate in candidates {
        let page = help_request(
            process.pid,
            &[
                "show",
                candidate["topic"].as_str().unwrap(),
                "--package",
                candidate["package"].as_str().unwrap(),
                "--help-key",
                candidate["help_key"].as_str().unwrap(),
            ],
        );
        assert_eq!(page["kind"], "page");
        assert_eq!(page["title"], candidate["title"]);
        assert!(!page["content"].as_str().unwrap().is_empty());
    }

    let target = arf_harp::help::HelpTarget::from_uri("x-r-help:base/%/%").unwrap();
    let page = help_request(
        process.pid,
        &[
            "show",
            &target.topic,
            "--package",
            target.package.as_deref().unwrap(),
        ],
    );
    assert_eq!(page["kind"], "page");
    assert_eq!(page["package"], "base");
    assert!(page["content"].as_str().unwrap().contains("%/%"));
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
