//! Smoke tests for the binary. These run against the executable Cargo built for this test run,
//! so with the `testkit` feature off they prove a feature-off binary rejects the scripted flag,
//! and with it on they prove the flag exists.
// Integration tests: unwrap is the honest way to fail loudly here.
#![allow(clippy::unwrap_used)]

use std::process::Command;

fn squint() -> Command {
    Command::new(env!("CARGO_BIN_EXE_squint"))
}

#[test]
fn version_prints_crate_version_and_commit() {
    let out = squint().arg("--version").output().expect("run squint");
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(env!("CARGO_PKG_VERSION")), "got {text}");
    let sha = text
        .trim()
        .rsplit_once('(')
        .map(|(_, rest)| rest.trim_end_matches(')'))
        .unwrap_or("");
    let hex9 = sha.len() == 9 && sha.chars().all(|c| c.is_ascii_hexdigit());
    assert!(
        hex9 || sha == "unknown",
        "expected a 9-char commit or 'unknown', got {text}"
    );
}

#[test]
fn no_command_is_a_usage_error() {
    let out = squint().output().expect("run squint");
    assert_eq!(out.status.code(), Some(64));
}

#[cfg(not(feature = "testkit"))]
#[test]
fn feature_off_binary_rejects_provider_script() {
    let out = squint()
        .args(["check", "--provider-script", "x.json"])
        .output()
        .expect("run squint");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unexpected argument"), "got {err}");
}

#[cfg(feature = "testkit")]
#[test]
fn feature_on_binary_lists_provider_script() {
    let out = squint()
        .args(["check", "--help"])
        .output()
        .expect("run squint");
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(help.contains("--provider-script"), "got {help}");
}
