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
