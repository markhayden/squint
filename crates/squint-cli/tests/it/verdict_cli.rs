//! rascal M13 / table-manners 2.4: every check action declares `answers: verdict`, and a
//! completed check carries the contract's `verdict`: its value, and in `where` the places tied
//! to the caller's own requirements (`RQ<n>`) that were violated or could not be told, on the
//! candidate. A finding about a built-in rule stays out of `where`. The calls are made exactly
//! as rascal's checker makes them: `squint call source_fidelity --arguments <file> --allow-dir
//! <dir>`, the images as `{"path": …}` inside the allowed directory, one `must_not` requirement.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Value, json};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// A fresh directory holding copies of the fixture pair, as rascal stages a try's images.
fn staged(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-verdict-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for name in ["candidate-64x48.png", "source-64x48.png"] {
        std::fs::copy(
            repo_root().join("fixtures/images").join(name),
            dir.join(name),
        )
        .unwrap();
    }
    dir.canonicalize().unwrap()
}

/// The checker's arguments as rascal's `[checker]` template renders them.
fn checker_arguments(dir: &Path, requirements: Value) -> Value {
    json!({
        "source": { "path": dir.join("source-64x48.png") },
        "candidate": { "path": dir.join("candidate-64x48.png") },
        "requirements": requirements,
    })
}

fn must_not() -> Value {
    json!([{ "id": "RQ1", "kind": "must_not", "text": "Any part of a white patch is visible." }])
}

/// `squint call source_fidelity` on the staged pair, the result read from stdout.
fn call(dir: &Path, arguments: &Value, script: &str) -> Value {
    let file = dir.join("args.json");
    std::fs::write(&file, arguments.to_string()).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(["call", "source_fidelity", "--arguments"])
        .arg(&file)
        .args([
            "--config",
            "fixtures/config/minimal.toml",
            "--provider-script",
            script,
        ])
        .arg("--allow-dir")
        .arg(dir)
        .arg("--out")
        .arg(dir.join("runs"))
        .output()
        .unwrap();
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not a result ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    })
}

#[test]
fn every_check_action_declares_that_it_answers_with_a_verdict() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(["describe", "--json"])
        .output()
        .unwrap();
    let d: Value = serde_json::from_slice(&o.stdout).unwrap();
    let actions = d["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 5);
    for a in actions {
        assert_eq!(a["answers"], "verdict", "{}", a["name"]);
    }
}

#[test]
fn a_held_requirement_passes_with_no_place() {
    let dir = staged("pass");
    let r = call(
        &dir,
        &checker_arguments(&dir, must_not()),
        "fixtures/scripts/requirement-held.json",
    );
    assert_eq!(r["status"], "completed");
    assert_eq!(r["verdict"], json!({ "value": "pass", "where": [] }));
}

#[test]
fn a_violated_requirement_fails_with_its_place_on_the_candidate() {
    let dir = staged("fail");
    let r = call(
        &dir,
        &checker_arguments(&dir, must_not()),
        "fixtures/scripts/intent-must-not-violation.json",
    );
    assert_eq!(r["status"], "completed", "a fail is a completed run");
    assert_eq!(r["verdict"]["value"], "fail");
    let places = r["verdict"]["where"].as_array().unwrap();
    assert_eq!(places.len(), 1);
    assert_eq!(places[0]["kind"], "bbox");
    assert_eq!(places[0]["parts"][0]["bbox"], json!([0.1, 0.2, 0.3, 0.4]));
    assert_eq!(places[0]["source"]["item"], "RQ1");
    // The place is on the candidate the call named.
    let candidate = r["images"]["in"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["role"] == "candidate")
        .unwrap();
    assert_eq!(
        places[0]["image"]["file_sha256"],
        candidate["image"]["file_sha256"]
    );
}

#[test]
fn a_requirement_it_cannot_tell_is_unsure_with_its_place() {
    let dir = staged("unsure");
    let r = call(
        &dir,
        &checker_arguments(&dir, must_not()),
        "fixtures/scripts/requirement-cant-tell.json",
    );
    assert_eq!(r["verdict"]["value"], "unsure");
    assert_eq!(r["verdict"]["where"].as_array().unwrap().len(), 1);
}

#[test]
fn a_finding_about_a_built_in_rule_is_not_a_place() {
    let dir = staged("builtin");
    // No requirements of the caller's: the script's violation is on SF1, a built-in rule.
    let r = call(
        &dir,
        &checker_arguments(&dir, json!([])),
        "fixtures/scripts/finding-violation.json",
    );
    assert_eq!(r["verdict"]["value"], "fail");
    assert_eq!(r["verdict"]["where"], json!([]));
    assert!(
        !r["locations"]["found"].as_array().unwrap().is_empty(),
        "the finding is still reported, in locations.found"
    );
}
