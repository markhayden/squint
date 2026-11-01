//! Task 1.5a: the committed manifest and envelope JSON Schemas match the generated ones.
#![allow(clippy::unwrap_used)]

use squint_core::tool::{Envelope, Manifest};

fn check(name: &str, generated: serde_json::Value) {
    let text = serde_json::to_string_pretty(&generated).unwrap() + "\n";
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .join(name);
    if std::env::var_os("UPDATE_SCHEMAS").is_some() {
        std::fs::write(&path, &text).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        committed, text,
        "schemas/{name} is stale; run with UPDATE_SCHEMAS=1 to regenerate"
    );
}

#[test]
fn manifest_schema_is_current() {
    check("manifest-v1.json", Manifest::json_schema());
}

#[test]
fn envelope_schema_is_current() {
    check("envelope-v1.json", Envelope::json_schema());
}
