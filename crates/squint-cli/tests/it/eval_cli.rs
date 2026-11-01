//! Task 1.15 CLI scenario: `squint eval gate-table --fixture fixtures/stats/case-1.toml` prints the
//! interval and outcome for a synthetic result table.
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
fn eval_gate_prints_the_interval_and_outcome() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "eval",
            "gate-table",
            "--fixture",
            "fixtures/stats/case-1.toml",
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(out.contains("paired bootstrap: point 0.7500"), "{out}");
    assert!(
        out.contains("seed 20261026") && out.contains("5 of 6 families differ"),
        "{out}"
    );
    assert!(out.contains("outcome: admitted"), "{out}");
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "eval",
            "gate-table",
            "--fixture",
            "fixtures/stats/case-1.toml",
            "--json",
        ])
        .output()
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["verdict"]["outcome"], "admitted");
    assert_eq!(v["bootstrap"]["resamples"], 1000);
}
