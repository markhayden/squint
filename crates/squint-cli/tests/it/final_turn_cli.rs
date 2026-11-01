//! Task 1.7b CLI scenario: a script that keeps calling tools exhausts the turns, gets the final
//! instruction, calls once more, receives an error result and the repair, calls again, and the run
//! ends `error: no_submission`;
//! a tool crash followed by a valid submission still ends `error: tool`.
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

fn run(
    tag: &str,
    script: &str,
    tool_script: &str,
    extra: &[&str],
) -> (Option<i32>, serde_json::Value) {
    let out = std::env::temp_dir().join(format!(
        "squint-final-turn-cli-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let mut args = vec![
        "check",
        "--config",
        "fixtures/config/fake-tool.toml",
        "--job",
        "fixtures/jobs/pair.toml",
        "--provider-script",
        script,
        "--tool-script",
        tool_script,
        "--out",
        out.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(&args)
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
fn stray_final_call_ends_no_submission() {
    let (code, r) = run(
        "stray",
        "fixtures/scripts/stray-final-call.json",
        "fixtures/tool-scripts/regions-3-then-1.json",
        &["--set", "budget.tool_turns=1"],
    );
    assert_eq!(code, Some(3));
    assert_eq!(r["runs"][0]["error"]["class"], "no_submission");
    assert_eq!(
        r["runs"][0]["turns"].as_array().unwrap().len(),
        3,
        "final turn, repair, second stray"
    );
}

#[test]
fn crash_then_valid_submission_still_ends_error_tool() {
    let (code, r) = run(
        "crash",
        "fixtures/scripts/crash-then-submit.json",
        "fixtures/tool-scripts/empty.json",
        &[],
    );
    assert_eq!(code, Some(3));
    assert_eq!(r["runs"][0]["error"]["class"], "tool");
    assert!(r["verdict"].is_null());
}
