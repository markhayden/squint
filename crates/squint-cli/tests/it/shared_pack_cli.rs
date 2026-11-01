//! Task 4.4b at the binary: a pack configured `shared = true` serves every decision of an `eval
//! run` from one process, each decision's run under its own lease (the server is reset between
//! them), while a pack without the flag is started afresh per decision.
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

/// Runs a two-item fake matrix under arm `D_probe_state` with the fake pack, returning the
/// (pid, calls_since_reset) the probe reported in each decision.
fn probe_pids(tag: &str, shared: bool) -> Vec<(u64, u64)> {
    let world =
        std::env::temp_dir().join(format!("squint-shared-pack-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&world);
    std::fs::create_dir_all(&world).unwrap();
    let server = squint_testkit::mcp_server::path();
    let envelopes = repo_root().join("fixtures/tool-scripts/regions-3-then-1.json");
    std::fs::write(
        world.join("config.toml"),
        format!(
            r#"[provider]
profile = "fake"
model = "fake-model"

[tools.packs.fake]
command = {server:?}
args = ["--envelopes", {envelopes:?}]
shared = {shared}

[budget]
spend_cap_usd = 100.0

[privacy]
cloud_calls = true
account = "fake:test"

[output]
dir = "./squint-out"
"#,
            server = server.display().to_string(),
            envelopes = envelopes.display().to_string(),
        ),
    )
    .unwrap();
    std::fs::write(
        world.join("matrix.toml"),
        format!(
            r#"[matrix]
name = "shared-pack-{tag}"
config = {config:?}
sets = ["source-fidelity/pairs-v1"]
arms = ["D_probe_state"]
decision_runs = 1
repetitions = 1
spend_cap_usd = 1000.0

[[matrix.models]]
profile = "fake"
model = "fake-model"
"#,
            config = world.join("config.toml").display().to_string(),
        ),
    )
    .unwrap();
    std::fs::write(
        world.join("script.json"),
        serde_json::to_string(&serde_json::json!([
            { "tool_calls": [{ "id": "c1", "name": "probe_state", "arguments": {} }], "stop_reason": "tool_use" },
            { "tool_calls": [{ "id": "c_submit", "name": "submit_verdict", "arguments": {
                "schema_version": 1,
                "items": [
                    { "id": "chk:ob:SF1@candidate", "disposition": "held", "confidence": "confident", "reason": "scripted", "allowed_by": null, "location": null, "evidence": ["call:1"] },
                    { "id": "chk:ob:SF2@candidate", "disposition": "held", "confidence": "confident", "reason": "scripted", "allowed_by": null, "location": null, "evidence": [] }
                ],
                "findings": [],
                "opinion": { "verdict": "pass", "reason": "r" }
            } }], "stop_reason": "tool_use" }
        ]))
        .unwrap(),
    )
    .unwrap();
    let out = world.join("out");
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "eval",
            "run",
            "--matrix",
            world.join("matrix.toml").to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--evalsets-root",
            "fixtures/evalsets",
            "--provider-script",
            world.join("script.json").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let summary: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("summary.json")).unwrap()).unwrap();
    let decisions = summary["decisions"].as_array().unwrap();
    assert_eq!(decisions.len(), 2, "{summary}");
    decisions
        .iter()
        .map(|d| {
            assert_eq!(d["verdict"], "pass", "{d}");
            let dir = out.join(d["dir"].as_str().unwrap());
            let report: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap())
                    .unwrap();
            let call = &report["runs"][0]["tool_calls"][0];
            assert_eq!(call["tool"], "probe_state");
            assert_eq!(call["status"], "ok", "{call}");
            let sha = call["envelope"]["sha256"].as_str().unwrap();
            let envelope: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(dir.join("run-1/artifacts").join(sha)).unwrap(),
            )
            .unwrap();
            let get = |name: &str| {
                envelope["measurements"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|m| m["name"] == name)
                    .and_then(|m| m["value"].as_f64())
                    .unwrap() as u64
            };
            (get("pid"), get("calls_since_reset"))
        })
        .collect()
}

#[test]
fn a_shared_pack_serves_every_decision_from_one_process_under_separate_leases() {
    let probes = probe_pids("shared", true);
    assert_eq!(probes[0].0, probes[1].0, "one process: {probes:?}");
    assert!(
        probes.iter().all(|p| p.1 == 1),
        "each run starts from a reset: {probes:?}"
    );
}

#[test]
fn a_pack_that_is_not_shared_starts_afresh_per_decision() {
    let probes = probe_pids("per-decision", false);
    assert_ne!(probes[0].0, probes[1].0, "two processes: {probes:?}");
    assert!(probes.iter().all(|p| p.1 == 1), "{probes:?}");
}
