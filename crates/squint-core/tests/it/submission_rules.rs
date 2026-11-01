//! Task 1.6a: the canonical submission schema is committed and current; every semantic rule has a
//! failing fixture; a complete, correct submission validates.
#![allow(clippy::unwrap_used, clippy::collapsible_if)]

use std::path::PathBuf;

use squint_core::Error;
use squint_core::checklist::Checklist;
use squint_core::frames::Registry;
use squint_core::job::{Job, Projection, Role};
use squint_core::obligations::{MergeInput, Resolved, merge, resolve};
use squint_core::submission::{
    ValidationContext, canonical_schema, parse_and_validate, validate_schema,
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Fixture {
    checklist: Checklist,
    resolved: Resolved,
    registry: Registry,
    completed: Vec<String>,
}

impl Fixture {
    fn pip() -> Fixture {
        let job = Job::load(&root().join("fixtures/jobs/pip.toml")).unwrap();
        let merged = merge(&MergeInput {
            base: &job.use_case,
            profile: None,
            extra: &[],
            extra_names: &[],
            job: Some(&job),
            enabled_tools: &[],
        })
        .unwrap();
        let resolved = resolve(&merged, &job).unwrap();
        let images: Vec<(String, Role)> =
            job.images.iter().map(|i| (i.id.clone(), i.role)).collect();
        let checklist = Checklist::build(&resolved, &images).unwrap();
        let mut registry = Registry::new();
        for i in &job.images {
            registry.add_raw(&i.id, 64, 48, Projection::Flat);
        }
        Fixture {
            checklist,
            resolved,
            registry,
            completed: vec!["call:1".into()],
        }
    }

    fn ctx(&self) -> ValidationContext<'_> {
        ValidationContext {
            checklist: &self.checklist,
            resolved: &self.resolved,
            registry: &self.registry,
            completed_calls: &self.completed,
        }
    }

    /// A complete, valid submission: every item held, no findings.
    fn valid(&self) -> serde_json::Value {
        let items: Vec<serde_json::Value> = self
            .checklist
            .ids()
            .map(|id| serde_json::json!({ "id": id, "disposition": "held", "confidence": "confident", "reason": "fine", "allowed_by": null, "location": null, "evidence": [] }))
            .collect();
        serde_json::json!({ "schema_version": 1, "items": items, "findings": [], "opinion": { "verdict": "pass", "reason": "x" } })
    }
}

fn whole(frame: &str) -> serde_json::Value {
    serde_json::json!({ "frame": frame, "kind": "whole_image", "bbox": null, "polygon": null, "yaw": null, "pitch": null, "fov": null })
}

fn bbox(frame: &str, b: [f64; 4]) -> serde_json::Value {
    serde_json::json!({ "frame": frame, "kind": "bbox", "bbox": b, "polygon": null, "yaw": null, "pitch": null, "fov": null })
}

fn expect_err(f: &Fixture, v: &serde_json::Value, needle: &str) {
    match parse_and_validate(v, &f.ctx()) {
        Err(Error::Submission(m)) => assert!(m.contains(needle), "expected {needle:?} in {m:?}"),
        other => panic!("expected a submission error containing {needle:?}, got {other:?}"),
    }
}

fn item_mut<'a>(v: &'a mut serde_json::Value, id: &str) -> &'a mut serde_json::Value {
    v["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|i| i["id"] == id)
        .unwrap()
}

#[test]
fn committed_submission_schema_is_current_and_closed() {
    let generated = serde_json::to_string_pretty(&canonical_schema()).unwrap() + "\n";
    let path = root().join("schemas/submission-v1.json");
    if std::env::var_os("UPDATE_SCHEMAS").is_some() {
        std::fs::write(&path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        committed, generated,
        "schemas/submission-v1.json is stale; run with UPDATE_SCHEMAS=1"
    );
    // Every object schema is closed and every property required.
    fn walk(v: &serde_json::Value) {
        if let Some(o) = v.as_object() {
            if o.get("type") == Some(&serde_json::Value::String("object".into())) {
                if let Some(props) = o.get("properties").and_then(|p| p.as_object()) {
                    assert_eq!(
                        o.get("additionalProperties"),
                        Some(&serde_json::Value::Bool(false)),
                        "{v}"
                    );
                    let required: Vec<&str> = o["required"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|r| r.as_str().unwrap())
                        .collect();
                    for key in props.keys() {
                        assert!(
                            required.contains(&key.as_str()),
                            "property {key} is not required in {v}"
                        );
                    }
                }
            }
            o.values().for_each(walk);
        } else if let Some(a) = v.as_array() {
            a.iter().for_each(walk);
        }
    }
    walk(&canonical_schema());
}

#[test]
fn a_complete_submission_validates() {
    let f = Fixture::pip();
    parse_and_validate(&f.valid(), &f.ctx()).unwrap();
}

#[test]
fn schema_rejects_unknown_fields_and_wrong_versions() {
    let f = Fixture::pip();
    let mut v = f.valid();
    v["extra"] = serde_json::json!(1);
    assert!(validate_schema(&v).is_err());
    let mut v = f.valid();
    v["schema_version"] = serde_json::json!(2);
    expect_err(&f, &v, "schema_version");
    let mut v = f.valid();
    item_mut(&mut v, "chk:ob:RC1@candidate")["rule_ids"] = serde_json::json!(["RC1"]);
    assert!(
        validate_schema(&v).is_err(),
        "items carry no obligation ids"
    );
}

#[test]
fn unknown_missing_and_duplicate_items() {
    let f = Fixture::pip();
    let mut v = f.valid();
    item_mut(&mut v, "chk:ob:RC1@candidate")["id"] = serde_json::json!("chk:ob:ZZ9@candidate");
    expect_err(&f, &v, "unknown checklist item");
    let mut v = f.valid();
    v["items"].as_array_mut().unwrap().remove(0);
    expect_err(&f, &v, "exactly once, appeared 0");
    let mut v = f.valid();
    let dup = v["items"][0].clone();
    v["items"].as_array_mut().unwrap().push(dup);
    expect_err(&f, &v, "appeared 2");
}

#[test]
fn a_violation_needs_a_location_and_a_reason() {
    let f = Fixture::pip();
    let mut v = f.valid();
    item_mut(&mut v, "chk:ob:RC1@candidate")["disposition"] = serde_json::json!("violation");
    expect_err(&f, &v, "needs a location");
    let mut v = f.valid();
    item_mut(&mut v, "chk:ob:RC1@candidate")["reason"] = serde_json::json!("  ");
    expect_err(&f, &v, "reason must not be empty");
}

#[test]
fn allowed_change_requires_an_applicable_exemption() {
    let f = Fixture::pip();
    // RQ2 waives RC3 on the candidate: valid on RC3, not on RC1.
    let mut v = f.valid();
    let it = item_mut(&mut v, "chk:ob:RC3@candidate");
    it["disposition"] = serde_json::json!("allowed_change");
    it["allowed_by"] = serde_json::json!("RQ2");
    parse_and_validate(&v, &f.ctx()).unwrap();
    let mut v = f.valid();
    let it = item_mut(&mut v, "chk:ob:RC1@candidate");
    it["disposition"] = serde_json::json!("allowed_change");
    it["allowed_by"] = serde_json::json!("RQ2");
    expect_err(&f, &v, "does not waive RC1");
    let mut v = f.valid();
    let it = item_mut(&mut v, "chk:ob:RC3@candidate");
    it["disposition"] = serde_json::json!("allowed_change");
    it["allowed_by"] = serde_json::json!("RC4");
    expect_err(&f, &v, "RC4 waives nothing");
    expect_err(&f, &v, "the item is `held`");
    let mut v = f.valid();
    let it = item_mut(&mut v, "chk:ob:RC3@candidate");
    it["disposition"] = serde_json::json!("allowed_change");
    expect_err(&f, &v, "needs allowed_by");
    let mut v = f.valid();
    item_mut(&mut v, "chk:ob:RC3@candidate")["allowed_by"] = serde_json::json!("RQ2");
    expect_err(&f, &v, "only valid with disposition allowed_change");
    let mut v = f.valid();
    let it = item_mut(&mut v, "chk:ob:RC3@candidate");
    it["disposition"] = serde_json::json!("allowed_change");
    it["allowed_by"] = serde_json::json!("ZZ9");
    expect_err(&f, &v, "not an enabled exemption");
}

#[test]
fn observed_is_not_an_item_disposition() {
    let f = Fixture::pip();
    let mut v = f.valid();
    item_mut(&mut v, "chk:ob:RC1@candidate")["disposition"] = serde_json::json!("observed");
    expect_err(&f, &v, "finding disposition");
}

#[test]
fn evidence_must_name_a_completed_call() {
    let f = Fixture::pip();
    let mut v = f.valid();
    item_mut(&mut v, "chk:ob:RC1@candidate")["evidence"] = serde_json::json!(["call:1"]);
    parse_and_validate(&v, &f.ctx()).unwrap();
    item_mut(&mut v, "chk:ob:RC1@candidate")["evidence"] = serde_json::json!(["call:9"]);
    expect_err(&f, &v, "not a tool call that completed");
}

#[test]
fn location_rules_wrong_kind_for_projection_and_unknown_frame() {
    let f = Fixture::pip();
    let mut v = f.valid();
    let it = item_mut(&mut v, "chk:ob:RC1@candidate");
    it["disposition"] = serde_json::json!("violation");
    it["location"] = serde_json::json!({ "frame": "raw:candidate", "kind": "view", "bbox": null, "polygon": null, "yaw": 10.0, "pitch": 0.0, "fov": 60.0 });
    expect_err(&f, &v, "equirectangular");
    let mut v = f.valid();
    let it = item_mut(&mut v, "chk:ob:RC1@candidate");
    it["disposition"] = serde_json::json!("violation");
    it["location"] = whole("raw:nowhere");
    expect_err(&f, &v, "unknown frame");
    let mut v = f.valid();
    let it = item_mut(&mut v, "chk:ob:RC1@candidate");
    it["disposition"] = serde_json::json!("violation");
    it["location"] = bbox("raw:candidate", [0.5, 0.5, 0.2, 0.9]);
    expect_err(&f, &v, "x0 < x1");
}

fn finding(
    ids: Vec<&str>,
    dispo: &str,
    allowed_by: Option<&str>,
    loc: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({ "obligation_ids": ids, "disposition": dispo, "confidence": "confident", "reason": "r", "allowed_by": allowed_by, "location": loc, "evidence": [] })
}

#[test]
fn finding_rules() {
    let f = Fixture::pip();
    // A valid violation finding on the candidate citing RC1.
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(
        vec!["RC1"],
        "violation",
        None,
        bbox("raw:candidate", [0.1, 0.1, 0.3, 0.3])
    )]);
    parse_and_validate(&v, &f.ctx()).unwrap();
    // Violation citing only an observe obligation.
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(
        vec!["RC5"],
        "violation",
        None,
        whole("raw:candidate")
    )]);
    expect_err(&f, &v, "at least one mandatory obligation");
    // Observed citing a mandatory obligation.
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(
        vec!["RC1"],
        "observed",
        None,
        whole("raw:candidate")
    )]);
    expect_err(&f, &v, "cite observe rules only");
    // Observed, valid.
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(
        vec!["RC5"],
        "observed",
        None,
        whole("raw:candidate")
    )]);
    parse_and_validate(&v, &f.ctx()).unwrap();
    // Located on an image the obligation does not judge.
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(
        vec!["RC1"],
        "violation",
        None,
        whole("raw:ref:pip")
    )]);
    expect_err(&f, &v, "does not judge image");
    // Unknown obligation; exemption cited as an obligation.
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(
        vec!["ZZ9"],
        "violation",
        None,
        whole("raw:candidate")
    )]);
    expect_err(&f, &v, "unknown obligation");
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(
        vec!["RQ2"],
        "violation",
        None,
        whole("raw:candidate")
    )]);
    expect_err(&f, &v, "is an exemption");
    // allowed_change finding: RQ2 waives RC3 on the candidate.
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(
        vec!["RC3"],
        "allowed_change",
        Some("RQ2"),
        bbox("raw:candidate", [0.1, 0.1, 0.3, 0.3])
    )]);
    parse_and_validate(&v, &f.ctx()).unwrap();
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(
        vec!["RC1"],
        "allowed_change",
        Some("RQ2"),
        bbox("raw:candidate", [0.1, 0.1, 0.3, 0.3])
    )]);
    expect_err(&f, &v, "does not waive RC1");
    // Findings may not carry held or hidden.
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(vec!["RC1"], "held", None, whole("raw:candidate"))]);
    assert!(validate_schema(&v).is_err());
}

/// An exemption with `waives = []` states what may change and nothing more: naming it in
/// `allowed_by` or citing it as an obligation is rejected with the disposition to use instead.
#[test]
fn an_exemption_that_waives_nothing_is_explained_not_just_refused() {
    let f = Fixture::pip();
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(
        vec!["RC4"],
        "violation",
        None,
        whole("raw:candidate")
    )]);
    expect_err(&f, &v, "RC4 waives nothing");
    expect_err(&f, &v, "cite the obligation the change bears on");
    let mut v = f.valid();
    v["findings"] = serde_json::json!([finding(
        vec!["RC1"],
        "allowed_change",
        Some("RC4"),
        bbox("raw:candidate", [0.1, 0.1, 0.3, 0.3])
    )]);
    expect_err(&f, &v, "RC4 waives nothing");
}
