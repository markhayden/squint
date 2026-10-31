//! Task 1.10b CLI scenario: `report rebuild` on a cancelled run yields terminal status
//! `cancelled`; `report prune --older-than 0s` removes it.
#![cfg(all(feature = "testkit", unix))]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn decision_in(out: &Path) -> Option<PathBuf> {
    std::fs::read_dir(out)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_dir())
}

#[test]
fn rebuild_of_a_cancelled_run_then_prune() {
    let out = std::env::temp_dir().join(format!("squint-report-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let child = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/minimal.toml",
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            "fixtures/scripts/slow-retry.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    loop {
        if let Some(d) = decision_in(&out)
            && std::fs::read_to_string(d.join("run-1/journal.jsonl"))
                .map(|t| t.contains("\"event\":\"retry\""))
                .unwrap_or(false)
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the run never reached the retry"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let output = child.wait_with_output().unwrap();
    // SIGTERM exits 143 (task 7.4).
    assert_eq!(output.status.code(), Some(143));
    let decision = decision_in(&out).unwrap();

    // Rebuild from the journal, as if the written report were lost.
    std::fs::remove_file(decision.join("report.json")).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .args(["report", "rebuild", decision.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert!(stdout.contains("run-1: cancelled"), "{stdout}");
    assert!(stdout.contains("verdict: none"), "{stdout}");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(decision.join("report.json")).unwrap())
            .unwrap();
    assert_eq!(report["runs"][0]["terminal"], "cancelled");

    // Prune everything at least zero seconds old.
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .args([
            "report",
            "prune",
            "--out",
            out.to_str().unwrap(),
            "--older-than",
            "0s",
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!decision.exists());
    assert!(String::from_utf8_lossy(&o.stdout).contains("pruned 1 decision(s)"));
}
