use crate::config::schema::{generate_schema, schema_path, write_schema};

#[test]
fn test_schema_snapshot() {
    let schema = generate_schema();
    insta::with_settings!({snapshot_path => "../snapshots"}, {
        insta::assert_snapshot!("config_schema", schema);
    });
}

/// Snapshot test for the default configuration file.
/// This ensures we notice when the config structure changes.
#[test]
fn test_default_config_snapshot() {
    let config = crate::config::generate_default_config();
    insta::with_settings!({snapshot_path => "../snapshots"}, {
        insta::assert_snapshot!("default_config", config);
    });
}

#[test]
fn test_schema_matches_artifact() {
    let schema = generate_schema();
    let path = schema_path();

    // If the artifact file exists, verify it matches the generated schema
    if path.exists() {
        let contents = std::fs::read_to_string(&path).expect("Failed to read schema file");
        assert_eq!(
            schema, contents,
            "Schema file is out of date. Run the generate_schema_file test to update."
        );
    }
}

/// Generate the schema file in artifacts/.
/// Run with: cargo test -p arf-console generate_schema_file -- --ignored
#[test]
#[ignore]
fn generate_schema_file() {
    write_schema().expect("Failed to write schema file");
    println!("Schema written to {:?}", schema_path());
}

#[test]
fn test_schema_is_valid_json() {
    let schema = generate_schema();
    let parsed: serde_json::Value =
        serde_json::from_str(&schema).expect("Schema should be valid JSON");

    // Verify it has expected top-level fields
    assert!(
        parsed.get("$schema").is_some(),
        "Schema should have $schema field"
    );
    assert!(
        parsed.get("title").is_some(),
        "Schema should have title field"
    );
    assert!(
        parsed.get("type").is_some(),
        "Schema should have type field"
    );
    assert!(
        parsed.get("properties").is_some(),
        "Schema should have properties field"
    );
}

#[test]
fn test_schema_has_new_structure() {
    let schema = generate_schema();
    let parsed: serde_json::Value =
        serde_json::from_str(&schema).expect("Schema should be valid JSON");

    let properties = parsed
        .get("properties")
        .expect("Schema should have properties");

    let history = properties
        .get("history")
        .and_then(|value| value.get("$ref"))
        .and_then(|value| value.as_str())
        .and_then(|reference| reference.strip_prefix("#/$defs/"))
        .and_then(|name| parsed.get("$defs").and_then(|defs| defs.get(name)))
        .expect("Schema should define history");
    let history_properties = history
        .get("properties")
        .expect("History schema should have properties");
    assert!(history_properties.get("disabled").is_none());
    assert!(history_properties.get("dir").is_none());
    let mode = history_properties
        .get("mode")
        .expect("History mode should be present in schema");
    assert!(
        history
            .get("required")
            .and_then(|required| required.as_array())
            .is_none_or(|required| !required.iter().any(|name| name == "mode")),
        "History mode must remain optional for the default persistent mode"
    );
    assert_eq!(mode["default"], "persistent");
    let variants = mode
        .get("oneOf")
        .and_then(|variants| variants.as_array())
        .expect("History mode should have string and object variants");
    assert!(variants.iter().any(|variant| {
        variant["type"] == "string"
            && variant["enum"]
                .as_array()
                .is_some_and(|values| values.iter().any(|value| value == "persistent"))
    }));
    assert!(variants.iter().any(|variant| {
        variant["type"] == "object"
            && variant["additionalProperties"] == false
            && variant["required"]
                .as_array()
                .is_some_and(|required| required.iter().any(|name| name == "dir"))
    }));

    // Should have startup section (contains r_source, show_banner, reprex)
    assert!(
        properties.get("startup").is_some(),
        "Schema should have startup section"
    );

    // Should have reprex section (contains static configuration)
    assert!(
        properties.get("reprex").is_some(),
        "Schema should have reprex section"
    );

    // Should have other sections
    assert!(
        properties.get("editor").is_some(),
        "Schema should have editor section"
    );
    assert!(
        properties.get("prompt").is_some(),
        "Schema should have prompt section"
    );
    assert!(
        properties.get("completion").is_some(),
        "Schema should have completion section"
    );
    assert!(
        properties.get("experimental").is_some(),
        "Schema should have experimental section"
    );

    // Should NOT have legacy sections or top-level fields that moved
    assert!(
        properties.get("general").is_none(),
        "Schema should NOT have general section"
    );
    assert!(
        properties.get("reprex").is_some(),
        "reprex should be in its top-level section"
    );
    assert!(
        properties.get("r_version").is_none(),
        "r_version should be in startup section, not top-level"
    );
    assert!(
        properties.get("show_banner").is_none(),
        "show_banner should be in startup section, not top-level"
    );
    assert!(
        properties.get("shortcuts").is_none(),
        "Schema should NOT have shortcuts section"
    );
    assert!(
        properties.get("formatter").is_none(),
        "Schema should NOT have formatter section"
    );
}

#[test]
fn test_schema_has_r_source_overrides() {
    let schema = generate_schema();
    let parsed: serde_json::Value =
        serde_json::from_str(&schema).expect("Schema should be valid JSON");

    let experimental = parsed
        .get("$defs")
        .and_then(|defs| defs.get("ExperimentalConfigSchema"))
        .and_then(|schema| schema.get("properties"))
        .expect("Schema should define experimental properties");
    assert!(
        experimental.get("r_source_overrides").is_some(),
        "Schema should have r_source_overrides"
    );
}
