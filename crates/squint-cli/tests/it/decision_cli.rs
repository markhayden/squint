//! Task 1.6b CLI scenarios: two decision runs combined by the table, both kept in the report, and
//! the consumer policy applied to the exit code only.
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

fn tempdir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("squint-decision-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(script: &str, out: &Path, extra: &[&str]) -> (Option<i32>, serde_json::Value, String) {
    let mut args = vec![
        "check",
        "--config",
        "fixtures/config/minimal.toml",
        "--job",
        "fixtures/jobs/pair.toml",
        "--provider-script",
        script,
        "--out",
        out.to_str().unwrap(),
        "--set",
        "budget.decision_runs=2",
    ];
    args.extend_from_slice(extra);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(&args)
        .output()
        .unwrap();
    let dir = std::fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    assert!(dir.join("run-1/journal.jsonl").exists() && dir.join("run-2/journal.jsonl").exists());
    (
        o.status.code(),
        report,
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
fn two_agreeing_passes_pass() {
    let out = tempdir("agree");
    let (code, r, err) = run("fixtures/scripts/two-runs-agree.json", &out, &[]);
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(r["decision_runs"], 2);
    assert_eq!(r["runs"].as_array().unwrap().len(), 2);
    assert_eq!(r["decision"]["agree"], true);
    assert_eq!(r["verdict"], "pass");
}

#[test]
fn a_pass_and_an_unsure_are_unsure_with_both_runs_kept() {
    let out = tempdir("unsure");
    let (code, r, err) = run("fixtures/scripts/two-runs-pass-then-unsure.json", &out, &[]);
    assert_eq!(code, Some(2), "{err}");
    assert_eq!(r["runs"][0]["verdict"], "pass");
    assert_eq!(r["runs"][1]["verdict"], "unsure");
    assert_eq!(r["verdict"], "unsure");
    assert_eq!(r["exit_code"], 2);
}

#[test]
fn any_fail_is_fail() {
    let out = tempdir("fail");
    let (code, r, _) = run("fixtures/scripts/two-runs-pass-then-fail.json", &out, &[]);
    assert_eq!(code, Some(1));
    assert_eq!(r["verdict"], "fail");
}

#[test]
fn policy_maps_unsure_to_the_exit_code_only() {
    let out = tempdir("policy");
    let (code, r, err) = run(
        "fixtures/scripts/two-runs-pass-then-unsure.json",
        &out,
        &["--set", "policy.on_unsure=\"pass\""],
    );
    assert_eq!(code, Some(0), "{err}");
    assert_eq!(r["verdict"], "unsure", "the raw verdict is kept");
    assert_eq!(r["policy"]["on_unsure"], "pass");
    assert_eq!(r["exit_code"], 0);
}

#[test]
fn a_run_that_errors_makes_the_decision_an_error_and_both_runs_are_kept() {
    let out = tempdir("pass-then-error");
    let (code, report, err) = run("fixtures/scripts/two-runs-pass-then-error.json", &out, &[]);
    assert_eq!(code, Some(3), "{err}");
    assert_eq!(report["decision"]["error"], true);
    assert!(report["verdict"].is_null());
    assert_eq!(report["runs"][0]["verdict"], "pass");
    assert_eq!(report["runs"][1]["error"]["class"], "submission_invalid");
    assert_eq!(report["runs"][1]["repairs"].as_array().unwrap().len(), 1);
}
