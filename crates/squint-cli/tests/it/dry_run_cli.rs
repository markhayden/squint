//! Task 1.4b CLI scenario: `check --dry-run` prints the rendered Obligations section, the
//! instances and the checklist without a provider.
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
fn dry_run_prints_obligations_instances_and_checklist() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/minimal.toml",
            "--job",
            "fixtures/jobs/pip.toml",
            "--dry-run",
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
    assert!(out.contains("Obligations."), "{out}");
    assert!(
        out.contains("RQ1@candidate") && out.contains("RC1@candidate"),
        "{out}"
    );
    assert!(out.contains("chk:ob:RQ1@candidate"), "{out}");
    assert!(
        out.contains("chk:ref:ref:pip") && out.contains("chk:ref:ref:bridge"),
        "{out}"
    );
    assert!(out.contains("evidence=[\"ref:pip\"]"), "{out}");
}

#[test]
fn dry_run_with_an_unconsulted_reference_is_a_usage_error() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/minimal.toml",
            "--dry-run",
            "--use-case",
            "source-fidelity",
            "--candidate",
            "fixtures/images/candidate-64x48.png",
            "--source",
            "fixtures/images/source-64x48.png",
            "--ref",
            "stray=fixtures/images/two-to-one-128x64.png",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&o.stderr).contains("consulted by no mandatory obligation"));
}
