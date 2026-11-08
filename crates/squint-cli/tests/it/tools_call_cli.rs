//! `squint tools call`: one tool and its prerequisites on a pair of images, with no model.
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

fn tools_call(tool: &str, extra: &[&str], out: &Path) -> std::process::Output {
    let _ = std::fs::remove_dir_all(out);
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "tools",
            "call",
            tool,
            "--config",
            "fixtures/config/minimal.toml",
            "--use-case",
            "source-fidelity",
            "--source",
            "fixtures/images/align/scene.png",
            "--candidate",
            "fixtures/images/align/registered.png",
            "--out",
            out.to_str().unwrap(),
        ])
        .args(extra)
        .output()
        .unwrap()
}

fn out_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("squint-tools-call-{tag}-{}", std::process::id()))
}

#[test]
fn a_tool_that_requires_align_runs_it_first_and_compares_with_its_frame() {
    let out = out_dir("change-map");
    let o = tools_call("change_map", &["--json"], &out);
    assert_eq!(
        o.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let printed: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("result.json")).unwrap()).unwrap();
    assert_eq!(printed, written);
    let calls = written["calls"].as_array().unwrap();
    let tools: Vec<&str> = calls.iter().map(|c| c["tool"].as_str().unwrap()).collect();
    assert_eq!(tools, ["align", "change_map"]);
    assert_eq!(
        calls[0]["arguments"],
        serde_json::json!({ "image": "candidate", "against": "source" })
    );
    let aligned = calls[0]["images"].as_object().unwrap();
    let (frame, path) = aligned.iter().next().unwrap();
    assert!(frame.starts_with("aligned:candidate:"), "{frame}");
    assert!(Path::new(path.as_str().unwrap()).is_file());
    assert_eq!(calls[1]["arguments"]["against"], frame.as_str());
    assert_eq!(calls[1]["arguments"]["frame"], "raw:source");
    assert!(calls[1]["envelope"]["status"].is_string());
    assert!(out.join("journal.jsonl").is_file());
}

#[test]
fn a_missing_argument_with_no_default_and_an_unknown_tool_are_usage_errors() {
    let o = tools_call("resize", &[], &out_dir("resize"));
    assert_eq!(o.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&o.stderr).contains("long_edge"));

    let o = tools_call("align", &["--profile", "nope"], &out_dir("profile"));
    assert_eq!(o.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&o.stderr).contains("architecture"));

    // A prerequisite's record survives its dependant's failure: align cannot register a square
    // candidate onto a 4:3 source, and change_map refuses two frames that do not share a grid.
    let run = |candidate: &str, tag: &str| {
        let out = out_dir(tag);
        let _ = std::fs::remove_dir_all(&out);
        let o = Command::new(env!("CARGO_BIN_EXE_squint"))
            .current_dir(repo_root())
            .args([
                "tools",
                "call",
                "change_map",
                "--config",
                "fixtures/config/minimal.toml",
                "--use-case",
                "source-fidelity",
                "--source",
                "fixtures/images/align/scene.png",
                "--candidate",
                candidate,
                "--out",
                out.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(out.join("result.json")).unwrap())
                .unwrap();
        (o, written)
    };
    let (o, written) = run("fixtures/images/align/square.png", "no-grid");
    assert_eq!(
        o.status.code(),
        Some(64),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(written["calls"][0]["tool"], "align");
    // A same-size pair align cannot register (here an unrelated scene, as a strong restyle can
    // look) falls back to the raw candidate and runs.
    let (o, written) = run("fixtures/images/align/unrelated.png", "no-frame");
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(written["calls"][1]["arguments"]["against"], "raw:candidate");

    let o = tools_call("outline", &[], &out_dir("unknown"));
    assert_eq!(o.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&o.stderr).contains("no tool \"outline\""));

    // A frame the session never made is the caller's mistake, refused by the harness.
    let o = tools_call(
        "view",
        &["--args", r#"{"frame": "aligned:candidate:t9"}"#],
        &out_dir("bad-frame"),
    );
    assert_eq!(o.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&o.stderr).contains("unknown frame"));
}
