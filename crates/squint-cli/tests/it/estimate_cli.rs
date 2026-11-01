//! Task 7.6 CLI scenarios: `squint estimate` prints the worst-case cost of one check and whether
//! the cap covers it, from catalog prices with no credential; `--max-spend-usd` lowers the cap
//! and `check` then refuses before any request.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn squint(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(args)
        .env_remove("ANTHROPIC_API_KEY")
        .env("SQUINT_ANTHROPIC_ACCOUNT", "org-estimate")
        .output()
        .unwrap()
}

fn json(o: &Output) -> serde_json::Value {
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    serde_json::from_slice(&o.stdout).unwrap()
}

/// A real profile is estimated from the catalog's prices: no key is read or needed.
#[test]
fn a_real_profile_is_estimated_without_a_credential() {
    let e = json(&squint(&[
        "estimate",
        "--config",
        "examples/source-fidelity.toml",
        "--use-case",
        "source-fidelity",
        "--candidate",
        "fixtures/images/candidate-64x48.png",
        "--source",
        "fixtures/images/source-64x48.png",
    ]));
    assert_eq!(e["use_case"], "source-fidelity");
    let usd = e["envelope_usd"].as_f64().unwrap();
    assert!(usd > 0.0, "{e}");
    assert_eq!(e["fits"], usd <= e["cap_usd"].as_f64().unwrap());
}

#[test]
fn a_lower_cap_is_reported_and_a_bad_one_is_a_usage_error() {
    let args = [
        "estimate",
        "--config",
        "examples/source-fidelity.toml",
        "--use-case",
        "source-fidelity",
        "--candidate",
        "fixtures/images/candidate-64x48.png",
        "--source",
        "fixtures/images/source-64x48.png",
        "--max-spend-usd",
    ];
    let mut low = args.to_vec();
    low.push("0.000001");
    let e = json(&squint(&low));
    assert_eq!(e["cap_usd"], 0.000001);
    assert_eq!(e["fits"], false);
    let mut bad = args.to_vec();
    bad.push("0");
    assert_eq!(squint(&bad).status.code(), Some(64));
}

/// A check whose cap the call lowered below its envelope refuses before any request, and its
/// `run.json` says `budget`.
#[cfg(feature = "testkit")]
#[test]
fn a_check_under_a_lowered_cap_refuses_before_any_request() {
    let out = std::env::temp_dir().join(format!("squint-estimate-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let o = squint(&[
        "check",
        "--config",
        "fixtures/config/minimal.toml",
        "--job",
        "fixtures/jobs/pair.toml",
        "--provider-script",
        "fixtures/scripts/held.json",
        "--max-spend-usd",
        "0.000001",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(
        o.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let dir = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let run: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(run["status"], "error");
    assert_eq!(run["error"]["code"], "budget");
    assert_eq!(run["spend"]["usd"], 0.0);
}

/// The scripted provider prices its own requests, so a testkit estimate matches what the fake run
/// would reserve.
#[cfg(feature = "testkit")]
#[test]
fn the_scripted_provider_is_estimated_from_its_own_prices() {
    let e = json(&squint(&[
        "estimate",
        "--config",
        "fixtures/config/minimal.toml",
        "--job",
        "fixtures/jobs/pair.toml",
        "--provider-script",
        "fixtures/scripts/held.json",
    ]));
    assert!(e["envelope_usd"].as_f64().unwrap() > 0.0);
    assert_eq!(e["decision_runs"], 1);
}
