//! SPEC "Commands": exit codes for `check` are 0 pass, 1 fail, 2 unsure, 3 error or cancelled and
//! 64 usage error, after `[policy]` is applied. Each is produced here.
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

fn check(tag: &str, script: &str, extra: &[&str]) -> (Option<i32>, String) {
    let out = std::env::temp_dir().join(format!("squint-exit-codes-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
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
    ];
    args.extend_from_slice(extra);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(&args)
        .output()
        .unwrap();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stderr).to_string(),
    )
}

#[test]
fn exit_0_is_pass() {
    let (code, err) = check("pass", "fixtures/scripts/held.json", &[]);
    assert_eq!(code, Some(0), "{err}");
}

#[test]
fn exit_1_is_fail() {
    let (code, err) = check("fail", "fixtures/scripts/violation.json", &[]);
    assert_eq!(code, Some(1), "{err}");
}

#[test]
fn exit_2_is_unsure_under_the_review_policy() {
    let (code, err) = check(
        "unsure",
        "fixtures/scripts/two-runs-pass-then-unsure.json",
        &[
            "--set",
            "budget.decision_runs=2",
            "--set",
            "policy.on_unsure=\"review\"",
        ],
    );
    assert_eq!(code, Some(2), "{err}");
    // `review` is the default; a consumer may turn unsure into a failure.
    let (code, err) = check(
        "unsure-fail",
        "fixtures/scripts/two-runs-pass-then-unsure.json",
        &[
            "--set",
            "budget.decision_runs=2",
            "--set",
            "policy.on_unsure=\"fail\"",
        ],
    );
    assert_eq!(code, Some(1), "{err}");
}

#[test]
fn exit_3_is_an_error() {
    let (code, err) = check("error", "fixtures/scripts/missing-item.json", &[]);
    assert_eq!(code, Some(3), "{err}");
    assert!(err.contains("error:"), "{err}");
}

#[test]
fn exit_64_is_a_usage_error() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/does-not-exist.toml",
            "--job",
            "fixtures/jobs/pair.toml",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(64));
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/minimal.toml",
            "--job",
            "fixtures/jobs/pair.toml",
            "--set",
            "budget.tool_turns=lots",
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(64),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}
