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
