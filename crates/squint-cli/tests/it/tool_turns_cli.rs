//! Task 1.7a CLI scenario: two tool turns then a submission, with both turns, all calls and two
//! evidence items in the report.
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
fn two_tool_turns_show_in_the_report() {
    let out = std::env::temp_dir().join(format!("squint-tool-turns-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/fake-tool.toml",
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            "fixtures/scripts/two-tool-turns.json",
            "--tool-script",
            "fixtures/tool-scripts/regions-3-then-1.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let dir = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    let run = &r["runs"][0];
    assert_eq!(run["turns"].as_array().unwrap().len(), 3);
    assert_eq!(run["tool_calls"].as_array().unwrap().len(), 2);
    let ev: Vec<&str> = run["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .filter(|id| id.starts_with("chk:ev:"))
        .collect();
    assert_eq!(ev.len(), 3, "two from the first call, one from the second");
    assert!(
        dir.join("run-1/frames/raw_candidate.png").exists(),
        "the tool received the canonical image as a file"
    );
}
