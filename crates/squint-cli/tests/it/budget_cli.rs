//! Task 1.9 CLI scenario: `check` with `spend_cap_usd = 0.01` exits 3 with `error: budget` before
//! any request; the report shows the envelope.
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
fn a_one_cent_cap_refuses_the_envelope_before_any_request() {
    let out = std::env::temp_dir().join(format!("squint-budget-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/minimal.toml",
            "--set",
            "budget.spend_cap_usd=0.01",
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            "fixtures/scripts/held.json",
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
    assert!(err.contains("budget") && err.contains("envelope"), "{err}");
    let dir = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(r["runs"][0]["error"]["class"], "budget");
    assert!(r["runs"][0]["turns"].as_array().unwrap().is_empty());
    assert!(r["cost"]["envelope"]["usd"].as_f64().unwrap() > 0.01);
    assert_eq!(r["cost"]["cap_usd"], 0.01);
}
