//! Task 2.1 CLI scenario: `catalog show` prints an entry; a config naming a model the catalog does
//! not have, or a setting the model rejects, is a usage error before anything runs.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn squint(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn catalog_show_prints_an_entry_and_list_names_every_model() {
    let o = squint(&["catalog", "show", "claude-opus-5-5"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("claude-opus-5-5 (Claude Opus 5.5) via anthropic-messages"),
        "{out}"
    );
    assert!(out.contains("input $4.00, output $20.00"), "{out}");
    assert!(out.contains("final turn: refuse, tool_removal"), "{out}");
    assert!(
        out.contains("https://platform.claude.com/docs/en/about-claude/pricing"),
        "{out}"
    );
    let o = squint(&["catalog", "show", "claude-opus-5-5", "--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["prices"]["input_per_million"], 4.0);
    let o = squint(&["catalog", "list"]);
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("gemini-4-argon")
            && out.contains("gpt-6.1-terra")
            && out.contains("profiles: anthropic, google-ai-studio, openai"),
        "{out}"
    );
    let o = squint(&["catalog", "show", "claude-opus-9"]);
    assert_eq!(o.status.code(), Some(64));
}

#[test]
fn check_refuses_a_model_the_catalog_does_not_have_before_running() {
    let o = squint(&[
        "check",
        "--config",
        "fixtures/config/minimal.toml",
        "--set",
        "provider.profile=\"anthropic\"",
        "--set",
        "provider.model=\"claude-opus-9\"",
        "--job",
        "fixtures/jobs/pair.toml",
    ]);
    assert_eq!(
        o.status.code(),
        Some(64),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("not in the catalog"));
    let o = squint(&[
        "check",
        "--config",
        "fixtures/config/minimal.toml",
        "--set",
        "provider.profile=\"anthropic\"",
        "--set",
        "provider.model=\"claude-opus-5-5\"",
        "--set",
        "provider.controls.effort=\"ultra\"",
        "--job",
        "fixtures/jobs/pair.toml",
    ]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(64), "{err}");
    assert!(
        err.contains("not supported by \"claude-opus-5-5\""),
        "{err}"
    );
}

/// A build without the testkit feature refuses the scripted pair for a live check and admits it
/// for a dry run, which sends nothing.
#[cfg(not(feature = "testkit"))]
#[test]
fn check_without_testkit_refuses_the_scripted_pair_unless_dry_run() {
    let o = squint(&[
        "check",
        "--config",
        "fixtures/config/minimal.toml",
        "--job",
        "fixtures/jobs/pair.toml",
    ]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(64), "{err}");
    assert!(err.contains("test double"), "{err}");
    let o = squint(&[
        "check",
        "--config",
        "fixtures/config/minimal.toml",
        "--job",
        "fixtures/jobs/pair.toml",
        "--dry-run",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// A live model's dry run passes the catalog and wire checks for `submit_verdict` and the enabled
/// tools before printing the prompt.
#[test]
fn dry_run_with_a_live_model_passes_the_wire_check() {
    let o = squint(&[
        "check",
        "--config",
        "fixtures/config/minimal.toml",
        "--set",
        "provider.profile=\"anthropic\"",
        "--set",
        "provider.model=\"claude-opus-5-5\"",
        "--set",
        "privacy.account=\"anthropic:org-test\"",
        "--job",
        "fixtures/jobs/pair.toml",
        "--dry-run",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("Checklist:"));
}

/// Consent is bound to the profile: an account for another provider is refused before anything
/// runs, and the account id is not echoed.
#[test]
fn check_refuses_an_account_that_does_not_consent_to_the_profile() {
    let o = squint(&[
        "check",
        "--config",
        "fixtures/config/minimal.toml",
        "--set",
        "provider.profile=\"anthropic\"",
        "--set",
        "provider.model=\"claude-opus-5-5\"",
        "--set",
        "privacy.account=\"openai:org-55512\"",
        "--job",
        "fixtures/jobs/pair.toml",
        "--dry-run",
    ]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(64), "{err}");
    assert!(err.contains("needs the prefix `anthropic:`"), "{err}");
    assert!(!err.contains("org-55512"), "{err}");
}

/// Task 2.4 at the binary, since task 9.6 the readiness refusal: a live profile with no
/// credential in the environment stops with the usage exit code before anything is sent or any
/// run directory exists, naming the variable as a server's `not_ready` refusal does.
#[test]
fn check_without_a_credential_is_not_ready_with_exit_64() {
    let out = std::env::temp_dir().join(format!("squint-no-key-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .env("SQUINT_ANTHROPIC_ACCOUNT", "org-example")
        .env_remove("ANTHROPIC_API_KEY")
        .args([
            "check",
            "--config",
            "fixtures/config/live-anthropic.toml",
            "--use-case",
            "source-fidelity",
            "--candidate",
            "fixtures/images/candidate-64x48.png",
            "--source",
            "fixtures/images/source-64x48.png",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(64), "{err}");
    assert!(
        err.contains("not ready: ANTHROPIC_API_KEY missing"),
        "{err}"
    );
    assert!(!out.exists(), "no run directory");
}
