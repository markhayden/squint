//! Task 7.5 CLI scenario: `squint describe --json` prints the contract's description, naming the
//! credential variable of the configured profile and never a value.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn describe_prints_the_description_and_no_secret() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "describe",
            "--json",
            "--config",
            "examples/source-fidelity.toml",
        ])
        .env("ANTHROPIC_API_KEY", "sk-describe-secret-value")
        .env("SQUINT_ANTHROPIC_ACCOUNT", "org-describe-secret-account")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!stdout.contains("sk-describe-secret-value"));
    assert!(!stdout.contains("org-describe-secret-account"));
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["profiles"], serde_json::json!(["images"]));
    let sf = doc["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "source_fidelity")
        .unwrap();
    assert_eq!(
        sf["cost"]["needs_env"],
        serde_json::json!(["ANTHROPIC_API_KEY"])
    );
    // A real provider may spend (squint issue #2).
    assert_eq!(sf["cost"]["may_spend"], true);
}

/// The fake profile can never reach a paid provider: its checks declare no spend and no
/// credential (squint issue #2), so a client that refuses paid work can run them.
#[test]
fn the_fake_profile_declares_free_checks() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "describe",
            "--json",
            "--config",
            "fixtures/config/minimal.toml",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    let doc: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    for action in doc["actions"].as_array().unwrap() {
        assert_eq!(action["cost"]["may_spend"], false, "{}", action["name"]);
        assert_eq!(
            action["cost"]["needs_env"],
            serde_json::json!([]),
            "{}",
            action["name"]
        );
    }
}

#[test]
fn describe_without_a_config_is_a_usage_error() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(["describe", "--json"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(64));
}
