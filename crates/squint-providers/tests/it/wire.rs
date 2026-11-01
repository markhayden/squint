//! Task 2.2: for each adapter the transformed `submit_verdict` schema accepts the spec example, a
//! payload violating a removed constraint still fails canonical validation, an unsupported
//! construct is refused with the reason, and the committed snapshots match.
#![allow(clippy::unwrap_used)]

use serde_json::{Value, json};
use squint_core::provider::ToolDefinition;
use squint_core::submission;
use squint_core::tool::Manifest;
use squint_providers::catalog::{Capabilities, Catalog};
use squint_providers::wire::{Adapter, WIRE_VERSION, check_tool_set, normalize, transform};

const ADAPTERS: [(Adapter, &str); 3] = [
    (Adapter::AnthropicMessages, "claude-opus-5-5"),
    (Adapter::GeminiGenerateContent, "gemini-4-argon"),
    (Adapter::OpenaiResponses, "gpt-6.1-sol"),
];

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn caps(model: &str) -> Capabilities {
    Catalog::shipped()
        .unwrap()
        .model(model)
        .unwrap()
        .capabilities
        .clone()
}

/// The spec example: the scripted submission from `fixtures/scripts/finding-observed.json`.
fn example_submission() -> Value {
    let text =
        std::fs::read_to_string(root().join("fixtures/scripts/finding-observed.json")).unwrap();
    let script: Value = serde_json::from_str(&text).unwrap();
    script[0]["tool_calls"][0]["arguments"].clone()
}

fn accepts(schema: &Value, payload: &Value) -> bool {
    jsonschema::validator_for(schema).unwrap().is_valid(payload)
}

fn regions_manifest() -> ToolDefinition {
    let text = std::fs::read_to_string(root().join("fixtures/tools/regions.toml")).unwrap();
    Manifest::from_toml(&text).unwrap().tool_definition()
}

#[test]
fn wire_submit_verdict_accepts_the_spec_example_on_every_adapter() {
    let canonical = submission::canonical_schema();
    let example = example_submission();
    submission::validate_schema(&example).unwrap();
    for (adapter, model) in ADAPTERS {
        let wire = transform(adapter, &caps(model), "submit_verdict", &canonical).unwrap();
        assert_eq!(wire.wire_version, WIRE_VERSION);
        assert!(
            accepts(&wire.schema, &example),
            "{adapter:?}: {:?}",
            wire.changes
        );
        assert!(wire.schema.get("$schema").is_none() && wire.schema.get("title").is_none());
        // The wire schema is never tighter: whatever the canonical schema accepts, it accepts.
        let mut bad = example.clone();
        bad["items"][0]["disposition"] = json!("sideways");
        assert!(
            !accepts(&wire.schema, &bad),
            "{adapter:?}: enums still enforced"
        );
    }
}

#[test]
fn wire_removed_constraints_stay_in_canonical_validation() {
    // Anthropic strict mode accepts minItems of 0 or 1 only: a three-number bbox passes the wire
    // schema and fails canonical validation.
    let canonical = submission::canonical_schema();
    let wire = transform(
        Adapter::AnthropicMessages,
        &caps("claude-opus-5-5"),
        "submit_verdict",
        &canonical,
    )
    .unwrap();
    assert!(
        wire.changes
            .iter()
            .any(|c| c.contains("bbox") && c.contains("minItems")),
        "{:?}",
        wire.changes
    );
    let mut short = example_submission();
    short["findings"][0]["location"]["bbox"] = json!([0.1, 0.1, 0.3]);
    assert!(accepts(&wire.schema, &short));
    let err = submission::validate_schema(&short).unwrap_err();
    assert_eq!(err.class(), "submission_invalid");
    // The same holds for a string length on every adapter.
    let schema = json!({
        "type": "object", "additionalProperties": false, "required": ["label"],
        "properties": { "label": { "type": "string", "maxLength": 3 } }
    });
    let long = json!({ "label": "too long" });
    assert!(!accepts(&schema, &long));
    for (adapter, model) in ADAPTERS {
        let wire = transform(adapter, &caps(model), "t", &schema).unwrap();
        assert!(accepts(&wire.schema, &long), "{adapter:?}");
        assert!(
            wire.changes
                .iter()
                .any(|c| c.ends_with("maxLength: removed")),
            "{adapter:?}: {:?}",
            wire.changes
        );
    }
}

#[test]
fn wire_refuses_unrepresentable_constructs_with_the_reason() {
    let pattern_props = json!({
        "type": "object", "additionalProperties": false, "required": [],
        "properties": {}, "patternProperties": { "^x-": { "type": "string" } }
    });
    let recursive = json!({
        "type": "object", "additionalProperties": false, "required": ["node"],
        "properties": { "node": { "$ref": "#/$defs/Node" } },
        "$defs": { "Node": { "type": "object", "additionalProperties": false, "required": ["next"],
                             "properties": { "next": { "anyOf": [ { "$ref": "#/$defs/Node" }, { "type": "null" } ] } } } }
    });
    let external = json!({
        "type": "object", "additionalProperties": false, "required": ["a"],
        "properties": { "a": { "$ref": "https://example.com/a.json" } }
    });
    let open_map = json!({
        "type": "object", "additionalProperties": false, "required": ["tags"],
        "properties": { "tags": { "type": "object", "additionalProperties": { "type": "string" } } }
    });
    let root_array = json!({ "type": "array", "items": { "type": "string" } });
    let unknown_keyword = json!({
        "type": "object", "additionalProperties": false, "required": ["a"],
        "properties": { "a": { "type": "string", "contentMediaType": "text/plain" } }
    });
    for (adapter, model) in ADAPTERS {
        let c = caps(model);
        let err = transform(adapter, &c, "odd", &root_array).unwrap_err();
        assert!(
            err.to_string().contains("the root must be an object"),
            "{err}"
        );
        let err = transform(adapter, &c, "odd", &unknown_keyword).unwrap_err();
        assert!(
            err.to_string()
                .contains("\"contentMediaType\" is not in the wire subset"),
            "{err}"
        );
        let err = transform(adapter, &c, "odd", &pattern_props).unwrap_err();
        assert_eq!(err.class(), "resolve");
        assert!(
            err.to_string().contains("\"odd\"") && err.to_string().contains("patternProperties"),
            "{err}"
        );
        let err = transform(adapter, &c, "odd", &external).unwrap_err();
        assert!(err.to_string().contains("external reference"), "{err}");
        let recursion = transform(adapter, &c, "tree", &recursive);
        let map = transform(adapter, &c, "tags", &open_map);
        match adapter {
            Adapter::OpenaiResponses => {
                recursion.unwrap();
                assert!(map.unwrap_err().to_string().contains("cannot be closed"));
            }
            Adapter::AnthropicMessages => {
                assert!(recursion.unwrap_err().to_string().contains("recursive"));
                assert!(map.unwrap_err().to_string().contains("cannot be closed"));
            }
            Adapter::GeminiGenerateContent => {
                assert!(recursion.unwrap_err().to_string().contains("recursive"));
                let w = map.unwrap();
                assert_eq!(
                    w.schema["properties"]["tags"]["additionalProperties"]["type"],
                    "string"
                );
            }
        }
    }
}

#[test]
fn wire_openai_requires_every_property_and_normalize_restores_the_canonical_shape() {
    let c = caps("gpt-6.1-sol");
    let schema = json!({
        "type": "object", "additionalProperties": false, "required": ["image"],
        "properties": {
            "image": { "type": "string" },
            "against": { "type": "string", "description": "optional" },
            "where": { "$ref": "#/$defs/Loc" }
        },
        "$defs": { "Loc": { "type": "object", "additionalProperties": false, "required": ["k"],
                            "properties": { "k": { "oneOf": [ { "enum": ["a", "b"] }, { "const": "c" } ] } } } }
    });
    let wire = transform(Adapter::OpenaiResponses, &c, "t", &schema).unwrap();
    assert!(wire.strict);
    assert_eq!(
        wire.schema["required"],
        json!(["against", "image", "where"])
    );
    assert_eq!(
        wire.schema["properties"]["against"]["type"],
        json!(["string", "null"])
    );
    assert!(
        wire.schema["properties"]["where"]["anyOf"].is_array(),
        "{}",
        wire.schema
    );
    assert_eq!(
        wire.schema["$defs"]["Loc"]["properties"]["k"],
        json!({ "type": "string", "enum": ["a", "b", "c"] })
    );
    // The model fills the nullable slots with null; normalisation drops them and fixes casing.
    let payload = json!({ "image": "candidate", "against": null, "where": { "k": "a" } });
    assert!(accepts(&wire.schema, &payload));
    let mut cased = payload.clone();
    cased["where"]["k"] = json!("A");
    let canonical = normalize(&schema, cased);
    assert_eq!(
        canonical,
        json!({ "image": "candidate", "where": { "k": "a" } })
    );
    assert!(accepts(&schema, &canonical));
}

#[test]
fn wire_gemini_inlines_references_and_keeps_bounds() {
    let wire = transform(
        Adapter::GeminiGenerateContent,
        &caps("gemini-4-argon"),
        "submit_verdict",
        &submission::canonical_schema(),
    )
    .unwrap();
    assert!(!wire.strict);
    let text = wire.schema.to_string();
    assert!(
        !text.contains("$ref") && !text.contains("$defs"),
        "references inlined"
    );
    assert!(!text.contains("oneOf"));
    let bbox = &wire.schema["properties"]["findings"]["items"]["properties"]["location"]["properties"]
        ["bbox"];
    assert_eq!(bbox["minItems"], 4, "Gemini enforces item counts");
    assert!(bbox.get("format").is_none());
    assert!(accepts(&wire.schema, &example_submission()));
}

#[test]
fn wire_tool_set_limits_are_enforced_per_adapter() {
    let regions = regions_manifest();
    let submit = submission::submit_verdict_tool();
    for (adapter, model) in ADAPTERS {
        let set =
            check_tool_set(adapter, &caps(model), &[regions.clone(), submit.clone()]).unwrap();
        assert_eq!(set.len(), 2);
        assert_eq!(set[0].definition.name, "regions");
    }
    // Anthropic: 16 union-typed parameters across the set; the submission already uses 13.
    let mut props = serde_json::Map::new();
    for i in 0..4 {
        props.insert(format!("p{i}"), json!({ "type": ["string", "null"] }));
    }
    let unions = ToolDefinition {
        name: "many".into(),
        description: "d".into(),
        input_schema: json!({ "type": "object", "additionalProperties": false, "required": ["p0", "p1", "p2", "p3"], "properties": props }),
    };
    let err = check_tool_set(
        Adapter::AnthropicMessages,
        &caps("claude-opus-5-5"),
        &[unions.clone(), submit.clone()],
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("17 union-typed parameters"),
        "{err}"
    );
    check_tool_set(
        Adapter::OpenaiResponses,
        &caps("gpt-6.1-sol"),
        &[unions.clone(), submit.clone()],
    )
    .unwrap();
    let many: Vec<ToolDefinition> = (0..21)
        .map(|i| ToolDefinition {
            name: format!("t{i}"),
            ..regions.clone()
        })
        .collect();
    let err =
        check_tool_set(Adapter::AnthropicMessages, &caps("claude-opus-5-5"), &many).unwrap_err();
    assert!(err.to_string().contains("20 strict tools"), "{err}");
    // OpenAI: nesting deeper than ten levels.
    let mut deep = json!({ "type": "string" });
    for _ in 0..11 {
        deep = json!({ "type": "object", "additionalProperties": false, "required": ["n"], "properties": { "n": deep } });
    }
    let err = check_tool_set(
        Adapter::OpenaiResponses,
        &caps("gpt-6.1-sol"),
        &[ToolDefinition {
            name: "deep".into(),
            description: "d".into(),
            input_schema: deep,
        }],
    )
    .unwrap_err();
    assert!(err.to_string().contains("nesting depth"), "{err}");
}

#[test]
fn wire_committed_snapshots_match_the_transformation() {
    let canonical = submission::canonical_schema();
    for (adapter, model) in ADAPTERS {
        let wire = transform(adapter, &caps(model), "submit_verdict", &canonical).unwrap();
        let generated = serde_json::to_string_pretty(&wire).unwrap() + "\n";
        let path = root().join(format!(
            "fixtures/schemas/submit_verdict-{}.json",
            adapter.id()
        ));
        if std::env::var_os("UPDATE_SCHEMAS").is_some() {
            std::fs::write(&path, &generated).unwrap();
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            committed,
            generated,
            "{} is stale; run with UPDATE_SCHEMAS=1 to regenerate",
            path.display()
        );
    }
}

#[test]
fn wire_normalize_fills_required_nullable_fields_a_provider_left_out() {
    // Gemini omits null-valued fields: they come back where the canonical schema requires a
    // nullable property; a missing non-nullable property stays missing for the validator.
    let nullable = json!({
        "type": "object", "additionalProperties": false, "required": ["id", "allowed_by", "location"],
        "properties": {
            "id": { "type": "string" },
            "allowed_by": { "type": ["string", "null"] },
            "location": { "anyOf": [ { "$ref": "#/$defs/Loc" }, { "type": "null" } ] }
        },
        "$defs": { "Loc": { "type": "object", "additionalProperties": false, "required": ["frame", "fov"],
                            "properties": { "frame": { "type": "string" }, "fov": { "type": ["number", "null"] } } } }
    });
    let filled = normalize(
        &nullable,
        json!({ "id": "x", "location": { "frame": "raw:candidate" } }),
    );
    assert_eq!(
        filled,
        json!({ "id": "x", "allowed_by": null, "location": { "frame": "raw:candidate", "fov": null } })
    );
    assert!(accepts(&nullable, &filled));
    assert_eq!(
        normalize(&nullable, json!({ "allowed_by": null })),
        json!({ "allowed_by": null, "location": null }),
        "id is not nullable and stays missing"
    );
}

#[test]
fn wire_openai_strict_mode_gets_bare_references_and_unions_for_nullable_arrays() {
    // Both rules were learned from the live API: a `$ref` with a sibling description and a type
    // array on an array were rejected as invalid schemas.
    let wire = transform(
        Adapter::OpenaiResponses,
        &caps("gpt-6.1-sol"),
        "submit_verdict",
        &submission::canonical_schema(),
    )
    .unwrap();
    let finding_location = &wire.schema["$defs"]["Finding"]["properties"]["location"];
    assert_eq!(
        finding_location,
        &json!({ "$ref": "#/$defs/Location" }),
        "{finding_location}"
    );
    let bbox = &wire.schema["$defs"]["Location"]["properties"]["bbox"];
    assert!(bbox.get("type").is_none(), "{bbox}");
    assert_eq!(bbox["anyOf"][1], json!({ "type": "null" }));
    assert_eq!(bbox["anyOf"][0]["type"], "array");
    assert_eq!(bbox["anyOf"][0]["minItems"], 4);
    assert!(
        bbox.get("description").is_some(),
        "the description stays on the property"
    );
    // Primitive nullables keep the type array; Anthropic keeps both forms as they were.
    assert_eq!(
        wire.schema["$defs"]["Location"]["properties"]["fov"]["type"],
        json!(["number", "null"])
    );
    let anthropic = transform(
        Adapter::AnthropicMessages,
        &caps("claude-opus-5-5"),
        "submit_verdict",
        &submission::canonical_schema(),
    )
    .unwrap();
    assert_eq!(
        anthropic.schema["$defs"]["Location"]["properties"]["bbox"]["type"],
        json!(["array", "null"])
    );
    assert!(accepts(&wire.schema, &example_submission()));
}
