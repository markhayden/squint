//! SPEC "Code style": every type that crosses a boundary (config, rules, manifest, envelope,
//! submission, report) derives Serialize, Deserialize and JsonSchema, refuses unknown fields, and
//! carries a version field.
#![allow(clippy::unwrap_used)]

use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;

use squint_core::config::Config;
use squint_core::obligations::RulesFile;
use squint_core::report::Report;
use squint_core::submission::Submission;
use squint_core::tool::{Envelope, Manifest};

/// The traits are checked by the bounds; the schema shows the rest.
fn boundary<T: Serialize + DeserializeOwned + JsonSchema>(name: &str) {
    let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    assert_eq!(
        schema["additionalProperties"], false,
        "{name}: unknown fields must be refused"
    );
    let properties = schema["properties"].as_object().unwrap();
    // A nested header (`[rules] version = 1`) counts; follow one `$ref` into `$defs`.
    let nested = |v: &serde_json::Value| -> Option<serde_json::Value> {
        match v["$ref"].as_str() {
            Some(r) => r
                .strip_prefix("#/$defs/")
                .map(|name| schema["$defs"][name].clone()),
            None => Some(v.clone()),
        }
    };
    let versioned = properties.keys().any(|k| k.ends_with("version"))
        || properties.values().filter_map(nested).any(|v| {
            v["properties"]
                .as_object()
                .is_some_and(|p| p.keys().any(|k| k.ends_with("version")))
        });
    assert!(
        versioned,
        "{name}: no version field among {:?}",
        properties.keys().collect::<Vec<_>>()
    );
}

#[test]
fn boundary_types_derive_the_three_traits_refuse_unknown_fields_and_carry_a_version() {
    boundary::<Config>("Config");
    boundary::<RulesFile>("RulesFile");
    boundary::<Manifest>("Manifest");
    boundary::<Envelope>("Envelope");
    boundary::<Submission>("Submission");
    boundary::<Report>("Report");
}

#[test]
fn boundary_versions_are_validated() {
    let text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/config/minimal.toml"),
    )
    .unwrap();
    let err = Config::parse(&format!("config_version = 2\n{text}"), &[], &|_| None).unwrap_err();
    assert!(
        err.to_string().contains("config_version must be 1"),
        "{err}"
    );
    let ok = Config::parse(&text, &[], &|_| None).unwrap();
    assert_eq!(ok.config_version, 1);
}
