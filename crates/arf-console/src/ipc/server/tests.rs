use super::*;

#[test]
fn test_extract_body_http() {
    let http = b"POST / HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{\"jsonrpc\":\"2.0\"}";
    assert_eq!(extract_body(http), b"{\"jsonrpc\":\"2.0\"}");
}

#[test]
fn test_extract_body_raw_json() {
    let raw = b"{\"jsonrpc\":\"2.0\"}";
    assert_eq!(extract_body(raw), b"{\"jsonrpc\":\"2.0\"}");
}

/// Tests that dispatch_request rejects both evaluate and user_input
/// in alternate mode.
// Protects the process-global `IN_ALTERNATE_MODE` atomic.
#[tokio::test]
#[serial_test::serial]
async fn test_dispatch_rejects_in_alternate_mode() {
    use super::super::protocol::R_NOT_AT_PROMPT;

    /// Drop guard that resets global IPC state on scope exit (including panics).
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            super::super::set_in_alternate_mode(false);
        }
    }

    super::super::set_in_alternate_mode(true);
    let _guard = Guard;

    // evaluate should be rejected
    let (tx, _rx) = mpsc::channel();
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "evaluate".to_string(),
        params: serde_json::json!({"code": "1+1"}),
        id: Some(serde_json::json!(1)),
    };
    let response = dispatch_request(request, &tx).await;
    assert_eq!(response.error.unwrap().code, R_NOT_AT_PROMPT);

    // user_input should also be rejected
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "user_input".to_string(),
        params: serde_json::json!({"code": "print('hello')"}),
        id: Some(serde_json::json!(2)),
    };
    let response = dispatch_request(request, &tx).await;
    assert_eq!(response.error.unwrap().code, R_NOT_AT_PROMPT);

    // Cleanup handled by Guard drop
}

/// Tests that `session` returns arf-only success (not an error) in alternate mode,
/// with a context-appropriate `r_unavailable_reason`.
// Protects the process-global `IN_ALTERNATE_MODE` atomic and `SESSION_META`
// session-metadata cache.
#[tokio::test]
#[serial_test::serial]
async fn test_session_returns_arf_only_in_alternate_mode() {
    use super::super::protocol::SessionResult;

    /// Drop guard that resets global IPC state on scope exit (including panics).
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            super::super::set_in_alternate_mode(false);
        }
    }

    super::super::set_in_alternate_mode(true);
    let _guard = Guard;

    let (tx, _rx) = mpsc::channel();
    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "session".to_string(),
        params: serde_json::json!({}),
        id: Some(serde_json::json!(1)),
    };
    let response = dispatch_request(request, &tx).await;

    // Should be a success, not an error
    assert!(
        response.error.is_none(),
        "session should not return an error"
    );
    let result_value = response.result.expect("session should return a result");
    let result: SessionResult =
        serde_json::from_value(result_value).expect("should parse as SessionResult");

    // Should have arf info
    assert!(!result.arf_version.is_empty());
    assert!(result.pid > 0);

    // R info should be absent with an explanation
    assert!(
        result.r.is_none(),
        "R info should be null in alternate mode"
    );
    let reason = result
        .r_unavailable_reason
        .expect("should have r_unavailable_reason");
    assert!(
        reason.contains("alternate mode"),
        "reason should mention alternate mode, got: {reason}"
    );
    assert!(result.hint.is_some(), "should have a hint");
}

/// Tests that `session` returns arf-only info when the main thread channel
/// is broken (tx.send fails).
// Protects the process-global `IN_ALTERNATE_MODE` atomic and `SESSION_META`
// session-metadata cache.
#[tokio::test]
#[serial_test::serial]
async fn test_session_fallback_on_channel_failure() {
    use super::super::protocol::SessionResult;

    /// Drop guard that resets global IPC state on scope exit (including panics).
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            super::super::set_in_alternate_mode(false);
        }
    }

    super::super::set_in_alternate_mode(false);
    let _guard = Guard;

    // Create a channel and immediately drop the receiver so send() fails
    let (tx, _rx) = mpsc::channel();
    drop(_rx);

    let request = JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        method: "session".to_string(),
        params: serde_json::json!({}),
        id: Some(serde_json::json!(1)),
    };
    let response = dispatch_request(request, &tx).await;

    // Should be a success with arf-only info
    assert!(
        response.error.is_none(),
        "session should not return an error"
    );
    let result_value = response.result.expect("session should return a result");
    let result: SessionResult =
        serde_json::from_value(result_value).expect("should parse as SessionResult");

    assert!(result.r.is_none(), "R info should be null");
    assert!(
        result.r_unavailable_reason.is_some(),
        "should have r_unavailable_reason"
    );
}

/// Tests that `log_file` in `SessionResult` reflects what was passed to `set_session_meta`.
// Protects the process-global `SESSION_META` session-metadata cache.
#[test]
#[serial_test::serial]
fn test_session_result_includes_log_file() {
    // With log_file set
    super::super::set_session_meta(
        "/tmp/test.sock".to_string(),
        "2026-01-01T00:00:00+00:00".to_string(),
        Some("/opt/R/4.4.1/lib/R".to_string()),
        Some("/tmp/arf.log".to_string()),
        None,
    );
    let result = super::super::collect_session_result(false, "test");
    assert_eq!(result.log_file.as_deref(), Some("/tmp/arf.log"));

    // Without log_file
    super::super::set_session_meta(
        "/tmp/test.sock".to_string(),
        "2026-01-01T00:00:00+00:00".to_string(),
        None,
        None,
        None,
    );
    let result = super::super::collect_session_result(false, "test");
    assert_eq!(result.log_file, None);

    // Verify JSON serialization always includes the field
    let json = serde_json::to_value(&result).unwrap();
    assert!(
        json.get("log_file").is_some(),
        "log_file field should always be present in JSON"
    );
    assert!(
        json["log_file"].is_null(),
        "log_file should be null when not configured"
    );
}

/// Tests that `r_home` in `SessionResult` reflects what was passed to
/// `set_session_meta`.
// Protects the process-global `SESSION_META` session-metadata cache.
#[test]
#[serial_test::serial]
fn test_session_result_includes_r_home() {
    super::super::set_session_meta(
        "/tmp/test.sock".to_string(),
        "2026-01-01T00:00:00+00:00".to_string(),
        Some("/opt/R/4.4.1/lib/R".to_string()),
        None,
        None,
    );
    let result = super::super::collect_session_result(false, "test");
    assert_eq!(result.r_home.as_deref(), Some("/opt/R/4.4.1/lib/R"));

    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["r_home"], "/opt/R/4.4.1/lib/R");

    super::super::set_session_meta(
        "/tmp/test.sock".to_string(),
        "2026-01-01T00:00:00+00:00".to_string(),
        None,
        None,
        None,
    );
    let result = super::super::collect_session_result(false, "test");
    assert_eq!(result.r_home, None);

    let json = serde_json::to_value(&result).unwrap();
    assert!(json.get("r_home").is_some());
    assert!(json["r_home"].is_null());
}

/// Tests that `history_session_id` in `SessionResult` reflects what was passed to
/// `set_session_meta`.
// Protects the process-global `SESSION_META` session-metadata cache.
#[test]
#[serial_test::serial]
fn test_session_result_includes_history_session_id() {
    // With history_session_id set
    let session_id: i64 = 1_700_000_000_000_000_000;
    super::super::set_session_meta(
        "/tmp/test.sock".to_string(),
        "2026-01-01T00:00:00+00:00".to_string(),
        None,
        None,
        Some(session_id),
    );
    let result = super::super::collect_session_result(false, "test");
    assert_eq!(result.history_session_id, Some(session_id));

    // Verify JSON serialization includes the value
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["history_session_id"], session_id);

    // Without history_session_id (history initialization unavailable)
    super::super::set_session_meta(
        "/tmp/test.sock".to_string(),
        "2026-01-01T00:00:00+00:00".to_string(),
        None,
        None,
        None,
    );
    let result = super::super::collect_session_result(false, "test");
    assert_eq!(result.history_session_id, None);

    // Verify JSON serialization shows null
    let json = serde_json::to_value(&result).unwrap();
    assert!(
        json["history_session_id"].is_null(),
        "history_session_id should be null when not set"
    );
}

#[cfg(unix)]
mod socket_dir_tests {
    use super::super::{is_dir_safe, random_hex_suffix, select_socket_dir};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn random_hex_suffix_has_expected_format() {
        let suffix = random_hex_suffix();
        assert_eq!(suffix.len(), 16);
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn nonexistent_dir_is_safe() {
        let tmp = tempfile::tempdir().unwrap();
        let candidate = tmp.path().join("does-not-exist");
        assert!(is_dir_safe(&candidate));
    }

    #[test]
    fn dir_with_0700_is_safe() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("good");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(is_dir_safe(&dir));
    }

    #[test]
    fn dir_with_group_read_is_unsafe() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("leaky");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(!is_dir_safe(&dir));
    }

    #[test]
    fn dir_with_other_write_is_unsafe() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("world");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o702)).unwrap();
        assert!(!is_dir_safe(&dir));
    }

    #[test]
    fn symlink_is_unsafe() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("real");
        std::fs::create_dir(&target).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(!is_dir_safe(&link));
    }

    #[test]
    fn regular_file_is_unsafe() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("not-a-dir");
        std::fs::write(&file, "").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(!is_dir_safe(&file));
    }

    #[test]
    fn select_uses_existing_safe_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let existing = tmp.path().join("existing");
        std::fs::create_dir(&existing).unwrap();
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o700)).unwrap();
        let fallback = tmp.path().join("fallback");
        // First candidate already exists and is safe — should be selected.
        let (path, created) =
            select_socket_dir(12345, &[existing.clone(), fallback.clone()]).unwrap();
        assert!(path.contains("existing"));
        assert!(
            !created,
            "dir already existed, should not report as created"
        );
        assert!(!fallback.exists(), "fallback should not have been created");
    }

    #[test]
    fn select_uses_first_safe_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let good = tmp.path().join("good");
        let also_good = tmp.path().join("also-good");
        // Neither exists yet — both are safe, first should win.
        let (path, created) = select_socket_dir(12345, &[good.clone(), also_good.clone()]).unwrap();
        assert!(path.contains("good"));
        assert!(created, "dir did not exist, should report as created");
        assert!(good.exists(), "first candidate should have been created");
        assert!(
            !also_good.exists(),
            "second candidate should not have been created"
        );
    }

    #[test]
    fn select_skips_unsafe_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("bad");
        std::fs::create_dir(&bad).unwrap();
        std::fs::set_permissions(&bad, std::fs::Permissions::from_mode(0o777)).unwrap();
        let good = tmp.path().join("fallback");
        let (path, created) = select_socket_dir(12345, &[bad, good.clone()]).unwrap();
        assert!(path.contains("fallback"));
        assert!(created, "fallback dir should have been created");
    }

    #[test]
    fn select_returns_none_when_all_unsafe() {
        let tmp = tempfile::tempdir().unwrap();
        let bad1 = tmp.path().join("bad1");
        std::fs::create_dir(&bad1).unwrap();
        std::fs::set_permissions(&bad1, std::fs::Permissions::from_mode(0o777)).unwrap();
        let bad2 = tmp.path().join("bad2");
        std::fs::write(&bad2, "").unwrap(); // regular file, not a dir
        std::fs::set_permissions(&bad2, std::fs::Permissions::from_mode(0o700)).unwrap();
        let result = select_socket_dir(12345, &[bad1, bad2]);
        assert!(result.is_none());
    }
}
