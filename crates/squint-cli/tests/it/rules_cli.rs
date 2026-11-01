//! Task 1.4a CLI scenario: `rules show` prints the merged list with provenance; a bad `waives`
//! fails naming the rule.
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
fn rules_show_prints_merged_obligations_with_provenance() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "rules",
            "show",
            "--config",
            "fixtures/config/architecture.toml",
            "--job",
            "fixtures/jobs/pair.toml",
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
    assert!(
        out.contains("SF1")
            && out.contains("[profile:architecture]")
            && out.contains("hidden_ok=true"),
        "{out}"
    );
    assert!(
        out.contains("SF2") && out.contains("[base:source-fidelity]"),
        "{out}"
    );
    assert!(
        String::from_utf8_lossy(&o.stderr).is_empty(),
        "regions is enabled so no hint warning"
    );
}

#[test]
fn rules_show_json_round_trips() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "rules",
            "show",
            "--config",
            "fixtures/config/architecture.toml",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["profile"], "architecture");
    assert!(v["hints"]["regions"]["preserve"].is_array());
}

#[test]
fn a_consumer_file_with_a_bad_waives_fails_naming_the_rule() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "rules",
            "show",
            "--config",
            "fixtures/config/bad-rules.toml",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(64));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("MY1") && err.contains("ZZ9"), "{err}");
}
