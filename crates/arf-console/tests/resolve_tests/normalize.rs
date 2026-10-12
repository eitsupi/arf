pub(super) fn assert_structured_resolve_error(output: &std::process::Output, code: &str) {
    assert!(output.stdout.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    let error = value["error"].as_object().unwrap();
    assert_eq!(error["code"], code);
    assert!(error["message"].is_string());
    assert!(error["hint"].is_null());
    assert!(error["data"].is_null());
}

pub(super) fn assert_selected_by_nullable_fields_are_present(value: &serde_json::Value) {
    let selected_by = value["selected_by"].as_object().unwrap();
    for field in ["requested_r_home", "requested_version"] {
        assert!(
            selected_by.contains_key(field),
            "missing selected_by.{field}"
        );
    }

    let source = selected_by["source"].as_object().unwrap();
    for field in ["name", "path", "format", "key"] {
        assert!(
            source.contains_key(field),
            "missing selected_by.source.{field}"
        );
    }
}

pub(super) fn normalize_descriptor(value: &serde_json::Value) -> String {
    let mut normalized = value.clone();
    let descriptor = normalized.as_object_mut().unwrap();

    replace_non_null(descriptor.get_mut("cwd"), "<cwd>");
    if let Some(resolver) = descriptor
        .get_mut("resolver")
        .and_then(|resolver| resolver.as_object_mut())
    {
        replace_non_null(resolver.get_mut("version"), "<arf-version>");
    }

    if let Some(target) = descriptor
        .get_mut("target")
        .and_then(|target| target.as_object_mut())
    {
        replace_non_null(target.get_mut("r_home"), "<r-home>");
        replace_non_null(target.get_mut("r_binary"), "<r-binary>");
        // The fake rig fixture used by every snapshot with a resolved version
        // always reports 4.4.2, so retaining that value pins the fixture's
        // deterministic version selection.
    }

    if let Some(selected_by) = descriptor
        .get_mut("selected_by")
        .and_then(|selected_by| selected_by.as_object_mut())
    {
        replace_non_null(
            selected_by.get_mut("requested_r_home"),
            "<requested-r-home>",
        );
        if let Some(source) = selected_by
            .get_mut("source")
            .and_then(|source| source.as_object_mut())
        {
            replace_non_null(source.get_mut("path"), "<source-path>");
        }
    }

    if let Some(diagnostics) = descriptor
        .get_mut("diagnostics")
        .and_then(|diagnostics| diagnostics.as_array_mut())
    {
        for diagnostic in diagnostics {
            if let Some(diagnostic) = diagnostic.as_object_mut() {
                // A message that names a path names the diagnostic's own path,
                // so substitute that exact string. Recognizing paths by their
                // syntax instead cannot be made reliable: a pattern that ends a
                // Windows path at the wrong character produces a snapshot that
                // differs from the one committed on Unix, and drive letters,
                // `~`, UNC prefixes and spaces in paths each break it
                // differently.
                let path = diagnostic
                    .get("path")
                    .and_then(|path| path.as_str())
                    .map(str::to_owned);
                let message = diagnostic
                    .get("message")
                    .and_then(|message| message.as_str())
                    .map(str::to_owned);
                if let (Some(path), Some(message)) = (path.as_deref(), message) {
                    diagnostic.insert(
                        "message".to_owned(),
                        serde_json::Value::String(message.replace(path, "<path>")),
                    );
                }
                replace_non_null(diagnostic.get_mut("path"), "<diagnostic-path>");
            }
        }
    }

    serde_json::to_string_pretty(&normalized).unwrap()
}

/// The descriptor snapshots are shared across platforms, so normalizing a
/// diagnostic must not depend on how the platform spells a path. Windows CI is
/// the only place a regression here would surface, so exercise the shapes it
/// produces directly.
#[test]
fn normalize_descriptor_handles_paths_from_any_platform() {
    for path in [
        r"/tmp/.tmpABC/broken.toml",
        r"C:\Users\runner\AppData\Local\Temp\.tmpABC\broken.toml",
        r"\\server\share\broken.toml",
        r"C:\Program Files\arf config\broken.toml",
    ] {
        let value = serde_json::json!({
            "diagnostics": [{
                "code": "config.parse_failed",
                "severity": "warning",
                "message": format!("Failed to load config from {path}: TOML parse error"),
                "path": path,
            }],
        });
        let normalized = normalize_descriptor(&value);
        assert!(
            normalized.contains(r"<path>: TOML parse error"),
            "path {path} normalized to: {normalized}"
        );
        assert!(!normalized.contains("broken.toml"), "leaked {path}");
    }

    let absolute_path = r"C:\Users\runner\AppData\Local\Temp\.tmpABC\broken.toml";
    let home_relative_path = r"~\AppData\Local\Temp\.tmpABC\broken.toml";
    let value = serde_json::json!({
        "diagnostics": [{
            "code": "config.parse_failed",
            "severity": "warning",
            "message": format!("Failed to load config from {home_relative_path}: TOML parse error"),
            "path": absolute_path,
        }],
    });
    // Normalization can only substitute a spelling it is given, so it depends on
    // the producer using the same one in both fields. Pin that dependency: when
    // they disagree, the message keeps a path the snapshot cannot absorb, and the
    // snapshot turns platform-specific. This is why a diagnostic is built with
    // the raw path and masked only when rendered for a person. Windows is where
    // divergence would show, because its temporary directories sit under the
    // user profile and Unix ones do not.
    let normalized: serde_json::Value =
        serde_json::from_str(&normalize_descriptor(&value)).unwrap();
    assert_eq!(
        normalized["diagnostics"][0]["message"],
        serde_json::Value::String(format!(
            "Failed to load config from {home_relative_path}: TOML parse error"
        )),
        "expected the mismatched spelling to survive, so this test keeps \
         documenting why the two fields must agree"
    );
}

fn replace_non_null(value: Option<&mut serde_json::Value>, placeholder: &str) {
    if let Some(value) = value
        && !value.is_null()
    {
        *value = serde_json::Value::String(placeholder.to_owned());
    }
}

/// Assert a descriptor path using its `cwd` for relative expected paths.
///
/// On macOS, `getcwd` resolves the `/var` symlink to `/private/var`, while
/// paths returned by `tempfile` may still contain `/var`. Cwd-derived paths
/// must therefore be compared against the descriptor's `cwd`, not directly
/// against a path returned by `tempfile`.
pub(super) fn assert_descriptor_path(
    value: &serde_json::Value,
    actual: &serde_json::Value,
    expected_path: impl AsRef<std::path::Path>,
) {
    let cwd = std::path::Path::new(value["cwd"].as_str().unwrap());
    let expected_path = expected_path.as_ref();
    let expected = if expected_path.is_absolute() {
        expected_path.to_owned()
    } else {
        cwd.join(expected_path)
    };
    assert_eq!(
        actual,
        &serde_json::Value::String(expected.display().to_string())
    );
}
