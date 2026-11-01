//! The diagnostics page's waterfall: `viewer/timeline.js` turns a journal into rows, one per step
//! of the funnel, pairing a turn's send and receipt and a tool call's dispatch and return. Runs
//! under `node` when it is on the path; without it the check is reported as skipped.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn which_node() -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join("node"))
        .find(|p| p.is_file())
}

/// A journal with every kind of step the waterfall draws: the set-up points, a turn that calls
/// three tools (one runs, one is refused by the harness, one is refused by the tool), a turn whose
/// attempt fails, a repair, the submission and the verdict.
fn journal() -> String {
    let t = 1_000_000u64;
    let lines = [
        json!({ "seq": 0, "unix_ms": t, "event": "resolved", "data": { "images": [{}, {}], "instances": [{}, {}, {}] } }),
        json!({ "seq": 1, "unix_ms": t + 10, "event": "prepared", "data": { "prep_version": "1", "images": 2 } }),
        json!({ "seq": 2, "unix_ms": t + 20, "event": "reserved", "data": { "envelope": { "usd": 1.25 }, "cap_usd": 10.0 } }),
        json!({ "seq": 3, "unix_ms": t + 30, "event": "turn_sent", "data": { "turn": 1 } }),
        json!({ "seq": 4, "unix_ms": t + 1030, "event": "turn_received", "data": { "turn": 1, "stop_reason": "tool_use", "tool_calls": ["align", "view", "view"], "cost_usd": 0.01 } }),
        json!({ "seq": 5, "unix_ms": t + 1040, "event": "tool_called", "data": { "call_id": "call:1", "tool": "align", "executed": true } }),
        json!({ "seq": 6, "unix_ms": t + 1041, "event": "tool_called", "data": { "call_id": "call:2", "tool": "view", "executed": false, "invalid_args": "frame: unknown frame id \"x\"" } }),
        json!({ "seq": 7, "unix_ms": t + 1042, "event": "tool_called", "data": { "call_id": "call:3", "tool": "view", "executed": true } }),
        json!({ "seq": 8, "unix_ms": t + 1050, "event": "tool_called", "data": { "call_id": "call:3", "tool": "view", "executed": false, "invalid_args": "flat: send bbox" } }),
        json!({ "seq": 9, "unix_ms": t + 3040, "event": "tool_returned", "data": { "call_id": "call:1", "status": "ok", "regions": 0, "images": [{}], "added": [] } }),
        json!({ "seq": 10, "unix_ms": t + 3100, "event": "turn_sent", "data": { "turn": 2 } }),
        json!({ "seq": 11, "unix_ms": t + 3600, "event": "attempt_failed", "data": { "turn": 2, "attempts": 1, "class": "provider" } }),
        json!({ "seq": 12, "unix_ms": t + 3700, "event": "repair", "data": { "form": "reissue", "reason": "invalid_submission" } }),
        json!({ "seq": 13, "unix_ms": t + 4700, "event": "submission", "data": {} }),
        json!({ "seq": 14, "unix_ms": t + 4710, "event": "verdict", "data": { "verdict": "fail" } }),
    ];
    lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

#[test]
fn the_waterfall_pairs_turns_and_tool_calls_and_keeps_the_funnel_order() {
    let Some(node) = which_node() else {
        eprintln!("node is not on the path; the timeline check was skipped");
        return;
    };
    let dir = std::env::temp_dir().join(format!("squint-timeline-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let journal_path = dir.join("journal.jsonl");
    std::fs::write(&journal_path, journal()).unwrap();
    let script = dir.join("run.js");
    std::fs::write(
        &script,
        r#"const fs = require('fs');
require(process.argv[2]);
const t = globalThis.squintTimeline;
process.stdout.write(JSON.stringify(t.rows(t.parse(fs.readFileSync(process.argv[3], 'utf8')))));
"#,
    )
    .unwrap();
    let o = Command::new(node)
        .arg(&script)
        .arg(repo_root().join("crates/squint-cli/viewer/timeline.js"))
        .arg(&journal_path)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let rows: Vec<Value> = serde_json::from_slice(&o.stdout).unwrap();
    let summary: Vec<(String, String, u64, u64)> = rows
        .iter()
        .map(|r| {
            (
                r["label"].as_str().unwrap().to_string(),
                r["status"].as_str().unwrap().to_string(),
                r["start_ms"].as_u64().unwrap(),
                r["duration_ms"].as_u64().unwrap(),
            )
        })
        .collect();
    let expect =
        |l: &str, s: &str, start: u64, dur: u64| (l.to_string(), s.to_string(), start, dur);
    assert_eq!(
        summary,
        vec![
            expect("resolve", "ok", 0, 0),
            expect("prepare", "ok", 0, 10),
            expect("reserve", "ok", 10, 10),
            expect("turn 1", "ok", 30, 1000),
            expect("align call:1", "ok", 1040, 2000),
            expect("view call:2", "invalid_args", 1041, 0),
            expect("view call:3", "invalid_args", 1042, 8),
            expect("turn 2", "failed", 3100, 500),
            expect("repair", "warn", 3600, 100),
            expect("submission", "ok", 3700, 1000),
            expect("verdict", "ok", 4700, 10),
        ]
    );
    assert_eq!(rows[3]["cost_usd"], 0.01);
    assert_eq!(rows[3]["detail"], "tool_use: align, view, view");
    assert_eq!(
        rows[4]["detail"],
        "0 region(s), 1 image(s), 0 item(s) added"
    );
    assert_eq!(rows[6]["detail"], "flat: send bbox");
    assert_eq!(rows[10]["detail"], "fail");
    let _ = std::fs::remove_dir_all(&dir);
}
