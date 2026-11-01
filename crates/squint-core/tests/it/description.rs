//! Task 7.5: squint's description document (table-manners 3 and 11.2), valid against the
//! contract's schema, with one action per shipped use case and every word squint can emit; since
//! task 9.1 (v0.3), each action's readiness and render kinds, and no `estimate` action.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::aggregate::Confidence;
use squint_core::config::Config;
use squint_core::contract;
use squint_core::submission::FindingDisposition;
use table_manners::{
    Description, Profile, ReadinessState, RenderKind, RequirementKind, RequirementState,
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn described() -> (Description, serde_json::Value) {
    described_with(&contract::Executor::Paid {
        credential: Some(contract::CredentialEnv {
            name: "GEMINI_API_KEY".to_string(),
            present: true,
        }),
    })
}

fn described_with(executor: &contract::Executor) -> (Description, serde_json::Value) {
    let text = std::fs::read_to_string(root().join("fixtures/config/minimal.toml")).unwrap();
    let config = Config::parse(&text, &[], &|_| None).unwrap();
    let d = contract::description(&config, "0.9.0", executor).unwrap();
    let doc = serde_json::to_value(&d).unwrap();
    valid(&doc);
    (d, doc)
}

fn description_schema() -> jsonschema::Validator {
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
    jsonschema::options()
        .with_retriever(Embedded)
        .build(&schema)
        .unwrap()
}

fn valid(doc: &serde_json::Value) {
    let errors: Vec<String> = description_schema()
        .iter_errors(doc)
        .map(|e| e.to_string())
        .collect();
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
fn one_action_per_shipped_use_case_and_no_estimate() {
    let (d, doc) = described();
    let mut names: Vec<&str> = d.actions.iter().map(|a| a.name.as_str()).collect();
    names.sort_unstable();
    // `estimate` is reserved (table-manners 3, 5): a served tool, never an action.
    assert_eq!(
        names,
        [
            "brief_compliance",
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
    assert!(!doc.to_string().contains("needs_env"));
    assert!(!sf.purpose.is_empty());
    // Requirements are typed: an obligation's own schema, not "any object".
    let item = &sf.params_schema["properties"]["requirements"]["items"];
    assert_eq!(item["required"], serde_json::json!(["id", "kind"]));
    assert!(item["properties"]["kind"].is_object());
    // A paid action lets a host pass its cap: a number above zero (table-manners 4.5).
    assert_eq!(
        sf.params_schema["properties"]["max_spend_usd"],
        serde_json::json!({
            "type": "number",
            "exclusiveMinimum": 0,
            "description": "Lowers the configured per-call cap; never raises it."
        })
    );
}

/// The schema holds a paid action to declaring `max_spend_usd` above zero: one without it, or
/// with `minimum: 0`, does not validate (table-manners 4.5).
#[test]
fn a_paid_action_without_a_positive_max_spend_does_not_validate() {
    let (_, doc) = described();
    let schema = description_schema();
    let mut missing = doc.clone();
    missing["actions"][0]["params_schema"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("max_spend_usd");
    assert!(!schema.is_valid(&missing));
    let mut zero_allowed = doc.clone();
    zero_allowed["actions"][0]["params_schema"]["properties"]["max_spend_usd"] =
        serde_json::json!({ "type": "number", "minimum": 0 });
    assert!(!schema.is_valid(&zero_allowed));
    assert!(schema.is_valid(&doc));
}

/// Readiness from what the executor knows (table-manners 3.1): a paid provider's credential by
/// name, `present` or `missing`, never its value; the scripted fake needs nothing.
#[test]
fn each_action_is_ready_or_names_the_credential_it_lacks() {
    let paid = |present| {
        described_with(&contract::Executor::Paid {
            credential: Some(contract::CredentialEnv {
                name: "GEMINI_API_KEY".to_string(),
                present,
            }),
        })
        .0
    };
    for (present, state, requirement) in [
        (true, ReadinessState::Ready, RequirementState::Present),
        (false, ReadinessState::NeedsSetup, RequirementState::Missing),
    ] {
        for action in paid(present).actions {
            assert_eq!(action.readiness.state, state, "{}", action.name);
            let [r] = action.readiness.requirements.as_slice() else {
                panic!("{}: one requirement", action.name);
            };
            assert_eq!(
                (r.kind, r.name.as_str(), r.state),
                (RequirementKind::Credential, "GEMINI_API_KEY", requirement)
            );
            assert!(r.setup.as_deref().unwrap().contains("GEMINI_API_KEY"));
        }
    }
    let (free, _) = described_with(&contract::Executor::Free);
    for action in free.actions {
        assert_eq!(action.readiness.state, ReadinessState::Ready);
        assert!(action.readiness.requirements.is_empty());
        assert!(!action.cost.may_spend);
    }
}

/// A run shows as a server page and as a document (table-manners 3.2).
#[test]
fn the_renders_are_a_server_and_a_document() {
    let (d, _) = described();
    for action in &d.actions {
        let kinds: Vec<(&str, RenderKind, Option<u32>)> = action
            .renders
            .iter()
            .map(|r| (r.name.as_str(), r.kind, r.ready_timeout_s))
            .collect();
        assert_eq!(
            kinds,
            [
                ("report", RenderKind::Server, Some(10)),
                ("export", RenderKind::Document, None)
            ]
        );
        for render in &action.renders {
            assert_eq!(render.command[0], "squint");
        }
    }
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

/// A config that names its rules describes only that use case (review I6): the one action a
/// call can succeed with. A rules file that cannot be read is an error, not a silent description
/// of the shipped base.
#[test]
fn a_config_with_rules_describes_only_its_use_case() {
    let minimal = std::fs::read_to_string(root().join("fixtures/config/minimal.toml")).unwrap();
    let with_rules = |extra: &str| {
        let text = format!("{minimal}\n[rules]\nbase = \"source-fidelity\"\nextra = [{extra:?}]\n");
        Config::parse(&text, &[], &|_| None).unwrap()
    };
    let mine = root().join("fixtures/rules/mine.toml");
    let d = contract::description(
        &with_rules(mine.to_str().unwrap()),
        "0.9.0",
        &contract::Executor::Free,
    )
    .unwrap();
    valid(&serde_json::to_value(&d).unwrap());
    let names: Vec<&str> = d.actions.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["source_fidelity"]);
    let missing = root().join("fixtures/rules/no-such-rules.toml");
    assert!(
        contract::description(
            &with_rules(missing.to_str().unwrap()),
            "0.9.0",
            &contract::Executor::Free
        )
        .is_err()
    );
}

/// Every action's `params_schema` is a schema of its own (squint issue #2): it compiles as a
/// standalone document, and the obligation inside `requirements` resolves its definitions from
/// that document's root, so a requirement of an unknown kind is refused.
#[test]
fn every_params_schema_compiles_on_its_own() {
    let (d, _) = described();
    let good = serde_json::json!({ "requirements": [
        { "id": "RQ1", "kind": "must_hold", "text": "The logo stays." }
    ]});
    let bad = serde_json::json!({ "requirements": [
        { "id": "RQ1", "kind": "sometimes", "text": "The logo stays." }
    ]});
    for action in &d.actions {
        let schema = jsonschema::draft202012::new(&action.params_schema)
            .unwrap_or_else(|e| panic!("{}: {e}", action.name));
        assert!(schema.is_valid(&good), "{}", action.name);
        assert!(!schema.is_valid(&bad), "{}", action.name);
    }
}
