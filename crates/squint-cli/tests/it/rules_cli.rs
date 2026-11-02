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

fn squint(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .env("SQUINT_ANTHROPIC_ACCOUNT", "org-rules")
        .args(args)
        .output()
        .unwrap()
}

/// Task 11.3: the shipped sets, by use case, with what a job needs and their profiles.
#[test]
fn rules_list_names_every_shipped_set() {
    let o = squint(&["rules", "list"]);
    assert_eq!(o.status.code(), Some(0));
    let out = String::from_utf8_lossy(&o.stdout);
    for base in squint_core::obligations::SHIPPED_BASES {
        assert!(out.lines().any(|l| l.starts_with(base)), "{base}: {out}");
    }
    assert!(
        out.lines()
            .any(|l| l.starts_with("source-fidelity") && l.ends_with("architecture")),
        "{out}"
    );
}

#[test]
fn rules_list_of_a_use_case_prints_each_rule_and_how_to_change_it() {
    let o = squint(&["rules", "list", "reference-consistency"]);
    assert_eq!(o.status.code(), Some(0));
    let out = String::from_utf8_lossy(&o.stdout);
    for id in ["RC1", "RC2", "RC3", "RC4", "RC5"] {
        assert!(out.lines().any(|l| l.starts_with(id)), "{id}: {out}");
    }
    assert!(
        out.contains(
            "RC2   must_hold  scope: candidate  evidence: reference\n      Distinctive markings"
        ),
        "{out}"
    );
    assert!(out.contains("RC5   observe"), "{out}");
    for note in [
        "--must-hold",
        "--must-not",
        "--allow",
        "[rules] extra",
        "the same id replaces",
        "`disabled = true`",
    ] {
        assert!(out.contains(note), "{note}: {out}");
    }
    let o = squint(&["rules", "list", "not-a-use-case"]);
    assert_eq!(o.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&o.stderr).contains("reference-consistency"));
}

/// Task 11.3: `rules init` writes a config and a rules file that load and change nothing until
/// edited; an edit replaces or removes a base rule; a second `init` overwrites nothing.
#[test]
fn rules_init_writes_a_starter_that_loads_and_never_overwrites() {
    let dir = std::env::temp_dir().join(format!("squint-rules-init-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let out = dir.join("mine");
    let o = squint(&[
        "rules",
        "init",
        "reference-consistency",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let config = out.join("squint.toml");
    let rules = out.join("rules.toml");
    assert!(String::from_utf8_lossy(&o.stderr).contains("ANTHROPIC_API_KEY"));
    let show = |config: &Path| {
        let o = squint(&["rules", "show", "--config", config.to_str().unwrap()]);
        assert_eq!(
            o.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let before = show(&config);
    assert_eq!(
        before.matches("[base:reference-consistency]").count(),
        5,
        "every base rule, unchanged: {before}"
    );

    let mut text = std::fs::read_to_string(&rules).unwrap();
    text.push_str(
        "\n[[rule]]\nid = \"RC2\"\nkind = \"must_hold\"\ntext = \"The white chest patch is unchanged.\"\n\
         \n[[rule]]\nid = \"RC5\"\nkind = \"observe\"\ndisabled = true\n",
    );
    std::fs::write(&rules, &text).unwrap();
    let after = show(&config);
    assert!(
        after.lines().any(|l| l.starts_with("RC2")
            && l.contains("[file:")
            && l.contains("white chest patch")),
        "{after}"
    );
    assert!(!after.lines().any(|l| l.starts_with("RC5")), "{after}");

    let config_text = std::fs::read_to_string(&config).unwrap();
    let o = squint(&[
        "rules",
        "init",
        "reference-consistency",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&o.stderr).contains("never overwrites"));
    assert_eq!(std::fs::read_to_string(&config).unwrap(), config_text);
    assert_eq!(std::fs::read_to_string(&rules).unwrap(), text);
}

#[test]
fn rules_init_refuses_when_either_file_exists_and_writes_neither() {
    let dir = std::env::temp_dir().join(format!("squint-rules-init-one-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rules.toml"), "mine").unwrap();
    let o = squint(&[
        "rules",
        "init",
        "reality-checks",
        "--out",
        dir.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(64));
    assert!(!dir.join("squint.toml").exists());
    assert_eq!(
        std::fs::read_to_string(dir.join("rules.toml")).unwrap(),
        "mine"
    );
}
