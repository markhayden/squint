//! Task 1.13 CLI scenario: `check` with `[tools.packs.fake] command = "<path>"` lists and calls
//! the fake tool through MCP.
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

fn config_with_pack(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-mcp-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let server = squint_testkit::mcp_server::path();
    let envelopes = repo_root().join("fixtures/tool-scripts/regions-3-then-1.json");
    let config = format!(
        "[provider]\nprofile = \"fake\"\nmodel = \"fake-model\"\n\n[tools]\nenabled = [\"fake_regions\"]\n\n[tools.packs.fake]\ncommand = {:?}\nargs = [\"--envelopes\", {:?}]\n\n[privacy]\ncloud_calls = true\naccount = \"fake:test\"\n\n[output]\ndir = {:?}\n",
        server.display().to_string(),
        envelopes.display().to_string(),
        dir.join("out").display().to_string()
    );
    let path = dir.join("config.toml");
    std::fs::write(&path, config).unwrap();
    path
}

#[test]
fn tools_list_shows_the_pack_tool_from_mcp() {
    let config = config_with_pack("list");
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(["tools", "list", "--config", config.to_str().unwrap()])
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
}

#[test]
fn check_calls_the_pack_tool_through_mcp() {
    let config = config_with_pack("check");
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            config.to_str().unwrap(),
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            "fixtures/scripts/two-tool-turns.json",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(0), "{stderr}");
    let out_dir = config.parent().unwrap().join("out");
    let decision = std::fs::read_dir(&out_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(decision.join("report.json")).unwrap())
            .unwrap();
    let calls = report["runs"][0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|c| c["status"] == "ok" && !c["envelope"].is_null())
    );
    assert_eq!(report["verdict"], "pass");
}
