//! Task 7.5 CLI scenario: `squint describe --json` prints the contract's description, naming the
//! credential variable of the configured profile and never a value; since task 9.1, in each
//! action's readiness, `present` or `missing` (table-manners 3.1).
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
        sf["readiness"],
        serde_json::json!({ "state": "ready", "requirements": [{
            "kind": "credential",
            "name": "ANTHROPIC_API_KEY",
            "state": "present",
            "setup": "Set ANTHROPIC_API_KEY in the environment squint is started with."
        }]})
    );
    assert!(!stdout.contains("needs_env"));
    // A real provider may spend (squint issue #2).
    assert_eq!(sf["cost"]["may_spend"], true);
}

/// With the credential unset (or blank), every paid action needs setup and names the variable.
#[test]
fn describe_names_a_missing_credential() {
    for unset in [None, Some("  ")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_squint"));
        command.current_dir(repo_root()).args([
            "describe",
            "--json",
            "--config",
            "examples/source-fidelity.toml",
        ]);
        match unset {
            None => command.env_remove("ANTHROPIC_API_KEY"),
            Some(blank) => command.env("ANTHROPIC_API_KEY", blank),
        };
        let o = command
            .env("SQUINT_ANTHROPIC_ACCOUNT", "org-describe")
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(0));
        let doc: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        for action in doc["actions"].as_array().unwrap() {
            assert_eq!(action["readiness"]["state"], "needs_setup", "{unset:?}");
            assert_eq!(
                action["readiness"]["requirements"][0]["state"], "missing",
                "{unset:?}"
            );
            assert_eq!(
                action["readiness"]["requirements"][0]["name"],
                "ANTHROPIC_API_KEY"
            );
        }
    }
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
            action["readiness"],
            serde_json::json!({ "state": "ready", "requirements": [] }),
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
