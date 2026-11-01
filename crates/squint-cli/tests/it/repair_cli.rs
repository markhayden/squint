//! Task 1.8 CLI scenario: a truncated first response then a valid submission exits 0 with one
//! excluded turn and one reissue in the report; an invalid submission then a valid one exits 0 with
//! one repair.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn run(tag: &str, script: &str) -> (Option<i32>, serde_json::Value) {
    let out = std::env::temp_dir().join(format!("squint-repair-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/minimal.toml",
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            script,
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let dir = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let r =
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    (o.status.code(), r)
}

#[test]
fn a_truncated_turn_is_excluded_reissued_and_the_run_passes() {
    let (code, r) = run("truncated", "fixtures/scripts/truncated-then-submit.json");
    assert_eq!(code, Some(0));
    let run = &r["runs"][0];
    let turns = run["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0]["entered_history"], false);
    assert_eq!(turns[0]["stop_reason"], "max_tokens");
    assert_eq!(
        run["repairs"],
        serde_json::json!([{ "form": "reissue", "reason": "truncated", "after_turn": 1 }])
    );
    assert_eq!(r["verdict"], "pass");
}

#[test]
fn an_invalid_submission_is_repaired_once_and_the_run_passes() {
    let (code, r) = run("invalid", "fixtures/scripts/invalid-then-submit.json");
    assert_eq!(code, Some(0));
    let run = &r["runs"][0];
    assert_eq!(run["turns"].as_array().unwrap().len(), 2);
    assert_eq!(
        run["repairs"],
        serde_json::json!([{ "form": "rejected", "reason": "invalid_submission", "after_turn": 1 }])
    );
    assert_eq!(r["verdict"], "pass");
}
