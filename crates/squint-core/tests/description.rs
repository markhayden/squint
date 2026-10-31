//! Task 7.5: squint's description document (table-manners 3 and 11.2), valid against the
//! contract's schema, with one action per shipped use case and every word squint can emit.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::aggregate::Confidence;
use squint_core::config::Config;
use squint_core::contract;
use squint_core::submission::FindingDisposition;
use table_manners::{Description, Profile};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn described() -> (Description, serde_json::Value) {
    let text = std::fs::read_to_string(root().join("fixtures/config/minimal.toml")).unwrap();
    let config = Config::parse(&text, &[], &|_| None).unwrap();
    let d = contract::description(&config, "0.9.0", &["GEMINI_API_KEY".to_string()]);
    let doc = serde_json::to_value(&d).unwrap();
    valid(&doc);
    (d, doc)
}

fn valid(doc: &serde_json::Value) {
    struct Embedded;
    impl jsonschema::Retrieve for Embedded {
        fn retrieve(
            &self,
            uri: &jsonschema::Uri<String>,
        ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
            let (_, text) = table_manners::SCHEMAS
                .iter()
                .find(|(id, _)| *id == uri.as_str())
                .ok_or("unknown schema")?;
            Ok(serde_json::from_str(text)?)
        }
    }
    let (_, text) = table_manners::SCHEMAS
        .iter()
        .find(|(id, _)| *id == "urn:table-manners:core:description")
        .unwrap();
    let schema: serde_json::Value = serde_json::from_str(text).unwrap();
    let validator = jsonschema::options()
        .with_retriever(Embedded)
        .build(&schema)
        .unwrap();
    let errors: Vec<String> = validator.iter_errors(doc).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{errors:?}\n{doc:#}");
}

/// The words of a serde enum, from its JSON Schema, so a new variant fails this test until the
/// description lists it.
fn words<T: schemars::JsonSchema>() -> Vec<String> {
    let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    let mut out: Vec<String> = schema["enum"]
        .as_array()
        .or_else(|| schema["oneOf"].as_array())
        .unwrap()
        .iter()
        .flat_map(|v| match v {
            serde_json::Value::String(s) => vec![s.clone()],
            other => other["enum"]
                .as_array()
                .map(|a| a.iter().map(|s| s.as_str().unwrap().to_string()).collect())
                .unwrap_or_else(|| vec![other["const"].as_str().unwrap().to_string()]),
        })
        .collect();
    out.sort();
    out
}

#[test]
fn the_description_is_the_contracts_with_the_images_profile() {
    let (d, _) = described();
    assert_eq!(d.contract, table_manners::CONTRACT_VERSION);
    assert_eq!(d.profiles, [Profile::Images]);
    assert_eq!(
        (d.tool.name.as_str(), d.tool.version.as_str()),
        ("squint", "0.9.0")
    );
    assert_eq!(d.progress_interval_s, 10);
    assert!(d.resources.contains(&"squint://runs".to_string()));
    assert!(
        d.resources
            .contains(&"squint://run/{id}/run.json".to_string())
    );
}

#[test]
fn one_action_per_shipped_use_case_and_estimate() {
    let (d, _) = described();
    let mut names: Vec<&str> = d.actions.iter().map(|a| a.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "brief_compliance",
            "estimate",
            "product_fidelity",
            "reality_checks",
            "reference_consistency",
            "source_fidelity"
        ]
    );
    let sf = d
        .actions
        .iter()
        .find(|a| a.name == "source_fidelity")
        .unwrap();
    let slots: Vec<(&str, bool)> = sf
        .images
        .as_ref()
        .unwrap()
        .iter()
        .map(|s| (s.slot.as_str(), s.required))
        .collect();
    // Any job may add references; source fidelity requires none.
    assert_eq!(
        slots,
        [("candidate", true), ("source", true), ("references", false)]
    );
    assert!(sf.cost.may_spend);
    assert_eq!(sf.cost.needs_env, ["GEMINI_API_KEY"]);
    assert!(!sf.purpose.is_empty());
    // Requirements are typed: an obligation's own schema, not "any object".
    let item = &sf.params_schema["properties"]["requirements"]["items"];
    assert_eq!(item["required"], serde_json::json!(["id", "kind"]));
    assert!(item["properties"]["kind"].is_object());
    let estimate = d.actions.iter().find(|a| a.name == "estimate").unwrap();
    assert!(!estimate.cost.may_spend);
}

#[test]
fn the_vocabulary_lists_every_word_squint_emits() {
    let (d, _) = described();
    let v = d.vocabulary.unwrap();
    let mut disposition = v.disposition.clone();
    disposition.sort();
    assert_eq!(disposition, words::<FindingDisposition>());
    let mut confidence = v.confidence.clone();
    confidence.sort();
    assert_eq!(confidence, words::<Confidence>());
    assert!(v.evidence.is_empty() && v.score_kind.is_empty());
}

#[test]
fn the_inline_limit_is_the_decode_bound() {
    let (d, _) = described();
    let text = std::fs::read_to_string(root().join("fixtures/config/minimal.toml")).unwrap();
    let config = Config::parse(&text, &[], &|_| None).unwrap();
    assert_eq!(
        d.limits.unwrap().inline_bytes,
        config.images.max_decode_bytes
    );
}
