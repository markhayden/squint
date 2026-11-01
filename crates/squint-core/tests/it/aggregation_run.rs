//! The aggregation rows reached through the whole state machine: findings (a confident violation
//! fails, an observation records), and instance flags that change a row (`matters = false`,
//! `hidden_ok = true`) against the default rules, including the single-run `unsure` exit code.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::run::{self, Services};
use squint_core::tool::ToolRegistry;
use squint_testkit::ScriptedProvider;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn check(
    tag: &str,
    config_file: &str,
    script: &str,
) -> (run::Outcome, serde_json::Value, Vec<serde_json::Value>) {
    let out = std::env::temp_dir().join(format!("squint-aggregation-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let text = std::fs::read_to_string(root().join("fixtures/config").join(config_file)).unwrap();
    let config = Config::parse(&text, &[], &|_| None).unwrap();
    let provider =
        ScriptedProvider::from_path(&root().join("fixtures/scripts").join(script)).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config,
        &job,
        &root(),
        &Services::new(&provider, &tools),
        &out,
        "test",
    ))
    .unwrap();
    let report =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    let journal = std::fs::read_to_string(o.dir.join("run-1/journal.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    (o, report, journal)
}

fn finding_rows(journal: &[serde_json::Value]) -> serde_json::Value {
    journal
        .iter()
        .find(|e| e["event"] == "submission")
        .map(|e| e["data"]["finding_rows"].clone())
        .unwrap()
}

#[test]
fn a_confident_violation_finding_fails_the_run() {
    let (o, r, j) = check(
        "finding-violation",
        "minimal.toml",
        "finding-violation.json",
    );
    assert_eq!(o.exit_code, 1, "{:?}", o.error);
    assert_eq!(r["verdict"], "fail");
    assert_eq!(finding_rows(&j), serde_json::json!(["fail"]));
    assert!(
        r["runs"][0]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["row"] == "recorded")
    );
}

#[test]
fn an_observed_finding_records_nothing_and_the_run_passes() {
    let (o, r, j) = check("finding-observed", "minimal.toml", "finding-observed.json");
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    assert_eq!(r["verdict"], "pass");
    assert_eq!(finding_rows(&j), serde_json::json!([]));
    let submission = j.iter().find(|e| e["event"] == "submission").unwrap();
    assert_eq!(submission["data"]["findings"], 1);
}

#[test]
fn cant_tell_on_an_instance_that_matters_is_unsure_with_exit_2_in_a_single_run() {
    let (o, r, _) = check("cant-tell-default", "minimal.toml", "cant-tell-sf1.json");
    assert_eq!(o.exit_code, 2, "{:?}", o.error);
    assert_eq!(r["verdict"], "unsure");
    assert_eq!(r["runs"][0]["items"][0]["row"], "unsure");
}

#[test]
fn cant_tell_on_an_instance_that_does_not_matter_is_recorded() {
    let (o, r, _) = check(
        "cant-tell-matters-off",
        "matters-off.toml",
        "cant-tell-sf2.json",
    );
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    assert_eq!(r["verdict"], "pass");
    assert_eq!(r["runs"][0]["items"][1]["row"], "recorded");
    // Against the default rules the same submission is unsure.
    let (o, _, _) = check(
        "cant-tell-sf2-default",
        "minimal.toml",
        "cant-tell-sf2.json",
    );
    assert_eq!(o.exit_code, 2);
}

#[test]
fn hidden_is_unsure_unless_the_instance_allows_it() {
    let (o, _, _) = check("hidden-default", "minimal.toml", "hidden-sf1.json");
    assert_eq!(o.exit_code, 2, "{:?}", o.error);
    let (o, r, _) = check("hidden-ok", "hidden-ok.toml", "hidden-sf1.json");
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    assert_eq!(r["runs"][0]["items"][0]["row"], "recorded");
}

#[test]
fn a_tentative_held_on_an_instance_that_matters_is_unsure() {
    let (o, r, _) = check("tentative-held", "minimal.toml", "tentative-held-sf1.json");
    assert_eq!(o.exit_code, 2, "{:?}", o.error);
    assert_eq!(r["runs"][0]["items"][0]["row"], "unsure");
}

/// Task 3.5a: the report carries each non-observed finding with its location as submitted and
/// mapped to the root image's raw frame, so localisation can be scored from the report alone.
#[test]
fn the_report_carries_findings_with_root_locations() {
    let (_, r, _) = check("finding-root", "minimal.toml", "finding-violation.json");
    let findings = r["runs"][0]["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1);
    let f = &findings[0];
    assert_eq!(f["obligation_ids"], serde_json::json!(["SF1"]));
    assert_eq!(f["disposition"], "violation");
    assert_eq!(f["confidence"], "confident");
    assert_eq!(f["location"]["frame"], "raw:candidate");
    assert_eq!(f["root"]["image"], "candidate");
    assert_eq!(f["root"]["frame"], "raw:candidate");
    assert_eq!(f["root"]["kind"], "bbox");
    assert_eq!(f["root"]["bbox"], f["location"]["bbox"]);
    assert_eq!(f["root"]["partial"], false);
    // An observed finding is recorded too, with no aggregation row behind it.
    let (_, r, _) = check(
        "finding-root-observed",
        "minimal.toml",
        "finding-observed.json",
    );
    let findings = r["runs"][0]["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["disposition"], "observed");
}
