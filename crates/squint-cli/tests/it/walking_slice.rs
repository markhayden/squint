//! Task 1.1: the thinnest valid end-to-end path. A pair job, one mandatory instance
//! (`SF2@candidate`), a scripted provider that submits one disposition, aggregation, a journal
//! and a report, and the exit code. Requires the `testkit` feature.
#![cfg(feature = "testkit")]
// Integration tests: unwrap is the honest way to fail loudly here.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn run_check(script: &str, out: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/minimal.toml",
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            &format!("fixtures/scripts/{script}"),
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("run squint")
}

fn report(out: &Path) -> serde_json::Value {
    let run_dir = std::fs::read_dir(out)
        .expect("output dir")
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .expect("one run directory");
    let text = std::fs::read_to_string(run_dir.join("report.json")).expect("report.json");
    serde_json::from_str(&text).expect("report parses")
}

fn journal_events(out: &Path) -> Vec<String> {
    let run_dir = std::fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let text = std::fs::read_to_string(run_dir.join("run-1/journal.jsonl")).expect("journal.jsonl");
    text.lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn held_disposition_passes_with_exit_0() {
    let out = tempdir("held");
    let o = run_check("held.json", &out);
    assert_eq!(
        o.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let r = report(&out);
    assert_eq!(r["report_version"], 1);
    assert_eq!(r["verdict"], "pass");
    let ids: Vec<&str> = r["runs"][0]["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        ["SF1@candidate", "SF2@candidate"],
        "the shipped source-fidelity base resolves two instances"
    );
    let sf2 = r["runs"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "chk:ob:SF2@candidate")
        .unwrap();
    assert_eq!(sf2["disposition"], "held");
    assert_eq!(sf2["row"], "recorded");
    assert_eq!(
        journal_events(&out),
        [
            "resolved",
            "prepared",
            "reserved",
            "turn_sent",
            "turn_received",
            "submission",
            "verdict"
        ]
    );
}

#[test]
fn confident_violation_fails_with_exit_1() {
    let out = tempdir("violation");
    let o = run_check("violation.json", &out);
    assert_eq!(
        o.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let r = report(&out);
    assert_eq!(r["verdict"], "fail");
    let sf2 = r["runs"][0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "chk:ob:SF2@candidate")
        .unwrap();
    assert_eq!(sf2["row"], "fail");
    assert_eq!(sf2["location"]["kind"], "whole_image");
}

#[test]
fn report_is_derived_from_an_append_only_journal() {
    let out = tempdir("journal");
    run_check("held.json", &out);
    let events = journal_events(&out);
    // Every event line is valid JSON with a monotonically non-decreasing sequence number.
    let run_dir = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let text = std::fs::read_to_string(run_dir.join("run-1/journal.jsonl")).unwrap();
    let seqs: Vec<u64> = text
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["seq"]
                .as_u64()
                .unwrap()
        })
        .collect();
    assert_eq!(seqs, (0..events.len() as u64).collect::<Vec<_>>());
}

fn tempdir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("squint-walking-slice-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
