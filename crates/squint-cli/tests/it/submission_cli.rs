//! Task 1.6a CLI scenario: a submission that omits a checklist item ends the run with
//! `error: submission_invalid` (the single repair arrives in task 1.8, so the first failure is
//! terminal for now).
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

#[test]
fn a_submission_missing_an_item_is_a_submission_error() {
    let out = std::env::temp_dir().join(format!("squint-submission-cli-{}", std::process::id()));
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
            "fixtures/scripts/missing-item.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("submission") && err.contains("chk:ob:SF2@candidate"),
        "{err}"
    );
    let run_dir = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("report.json")).unwrap())
            .unwrap();
    assert_eq!(report["runs"][0]["error"]["class"], "submission_invalid");
}
