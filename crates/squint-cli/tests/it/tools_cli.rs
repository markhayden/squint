//! Task 1.5a CLI scenario: `tools list` prints the fake tool's manifest as the model will see it.
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
fn tools_list_prints_the_enabled_manifests() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "tools",
            "list",
            "--config",
            "fixtures/config/fake-tool.toml",
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
    assert!(out.contains("fake_regions v0.0.1"), "{out}");
    assert!(out.contains("evidence kind: change_region"), "{out}");
    assert!(
        out.contains("model schema:") && out.contains("\"image\""),
        "{out}"
    );
    assert!(
        !out.contains("_squint"),
        "the model schema never mentions the harness key: {out}"
    );
}

#[test]
fn tools_list_refuses_an_unknown_enabled_tool() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "tools",
            "list",
            "--config",
            "fixtures/config/unknown-tool.toml",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&o.stderr).contains("not known to this build"));
}

#[test]
fn tools_list_refuses_a_provisioned_pack_that_is_not_installed() {
    let home = std::env::temp_dir().join(format!("squint-tools-cli-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(["tools", "list", "--config", "fixtures/config/full.toml"])
        .env("SQUINT_TEST_ACCOUNT", "acct")
        .env("XDG_DATA_HOME", &home)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(64));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("not installed") && err.contains("squint tools setup"),
        "{err}"
    );
}

#[test]
fn tools_list_shows_the_built_ins() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(["tools", "list", "--config", "fixtures/config/view.toml"])
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
        out.contains("view v1.0.0 (Cheap") && out.contains("resize v1.0.0 (Cheap"),
        "{out}"
    );
    assert!(out.contains("perspective view"), "{out}");
}
