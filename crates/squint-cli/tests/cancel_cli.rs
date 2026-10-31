//! Task 1.10a CLI scenario: a running `check` killed with SIGTERM (and SIGINT) ends its journal in
//! `cancelled` with the state reached; since task 7.4 it exits 143 (130) and leaves a `run.json`
//! with status `cancelled`.
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

fn journal_in(out: &Path) -> Option<PathBuf> {
    let decision = std::fs::read_dir(out)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.is_dir())?;
    let j = decision.join("run-1/journal.jsonl");
    j.exists().then_some(j)
}

fn killed_with(signal: &str, code: i32) {
    let out =
        std::env::temp_dir().join(format!("squint-cancel-cli-{signal}-{}", std::process::id()));
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
    // Wait until the run is sleeping before its retry.
    let started = Instant::now();
    loop {
        if let Some(j) = journal_in(&out)
            && std::fs::read_to_string(&j)
                .unwrap()
                .contains("\"event\":\"retry\"")
        {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the run never reached the retry"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = Command::new("kill")
        .args([format!("-{signal}"), child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("cancelled"), "{stderr}");
    let lines: Vec<serde_json::Value> = std::fs::read_to_string(journal_in(&out).unwrap())
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let last = lines.last().unwrap();
    assert_eq!(last["event"], "cancelled");
    assert_eq!(last["data"]["state"], "retry");
    let decision = journal_in(&out).unwrap();
    let run: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            decision
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("run.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(run["status"], "cancelled");
}

#[test]
fn sigterm_ends_the_journal_in_cancelled() {
    killed_with("TERM", 143);
}

#[test]
fn sigint_ends_the_journal_in_cancelled() {
    killed_with("INT", 130);
}
