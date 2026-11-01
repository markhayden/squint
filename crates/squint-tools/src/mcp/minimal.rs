//! Task 1.13: the lifecycle contract against the testkit's fake server.
#![allow(clippy::unwrap_used)]

use std::time::{Duration, Instant};

use serde_json::json;
use squint_core::tool::Status;
use squint_testkit::mcp_server;

use super::*;

fn envelopes() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/tool-scripts/regions-3-then-1.json")
        .display()
        .to_string()
}

fn spawn(args: &[&str]) -> Spawn {
    let mut all: Vec<String> = vec!["--envelopes".into(), envelopes()];
    all.extend(args.iter().map(|s| s.to_string()));
    Spawn {
        command: mcp_server::path(),
        args: all,
        declared_env: vec!["SQUINT_TEST_PLAIN".into(), "SQUINT_TEST_SECRET_KEY".into()],
        cwd: None,
        extra_env: vec![],
    }
}

fn timeouts() -> Timeouts {
    Timeouts {
        startup: Duration::from_secs(10),
        call: Duration::from_secs(5),
        shutdown_grace: Duration::from_millis(300),
    }
}

fn env() -> Vec<(String, String)> {
    let mut e: Vec<(String, String)> = std::env::vars().collect();
    e.push(("SQUINT_TEST_PLAIN".into(), "1".into()));
    e.push(("SQUINT_TEST_SECRET_KEY".into(), "SYNTHETIC".into()));
    e
}

fn context(run: &str) -> serde_json::Value {
    json!({ "run_id": run, "call_id": "call:1", "images": {}, "frames": {}, "hints": {} })
}

fn probe(pack: &Pack, run: &str) -> (u64, u64, String) {
    let e = pack
        .call(run, None, "probe_state", json!({ "_squint": context(run) }))
        .unwrap();
    let get = |name: &str| {
        e.measurements
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.value as u64)
            .unwrap()
    };
    (get("calls_since_reset"), get("pid"), e.summary.clone())
}

#[test]
fn wrong_protocol_version_is_refused_as_resolve() {
    let err = Client::connect(
        &spawn(&["--protocol-version", "2024-11-05"]),
        env(),
        Instant::now() + Duration::from_secs(10),
    )
    .err()
    .unwrap();
    assert_eq!(err.class(), "resolve", "{err}");
    assert!(err.to_string().contains("2025-11-25"), "{err}");
}

#[test]
fn missing_tools_capability_is_refused_as_resolve() {
    let err = Client::connect(
        &spawn(&["--no-tools-capability"]),
        env(),
        Instant::now() + Duration::from_secs(10),
    )
    .err()
    .unwrap();
    assert_eq!(err.class(), "resolve", "{err}");
    assert!(err.to_string().contains("tools"), "{err}");
}

#[test]
fn lists_tools_with_their_manifests_and_calls_with_the_context() {
    let pack = Pack::connect("fake", spawn(&[]), timeouts(), env()).unwrap();
    let names: Vec<&str> = pack.manifests().iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["fake_regions", "probe_state"]);
    assert_eq!(
        pack.manifests()[0].evidence_kind.as_deref(),
        Some("change_region")
    );
    let e = pack
        .call(
            "run-1",
            None,
            "fake_regions",
            json!({ "image": "candidate", "_squint": context("run-1") }),
        )
        .unwrap();
    assert_eq!(e.status, Status::Ok);
    assert_eq!(e.regions.len(), 3);
    // Without the context the server refuses the arguments: the one recoverable form.
    let e = pack
        .call(
            "run-1",
            None,
            "fake_regions",
            json!({ "image": "candidate" }),
        )
        .unwrap();
    assert_eq!(e.status, Status::InvalidArgs);
    // The environment reached the server filtered: the declared plain variable, not the key.
    let (_, _, summary) = probe(&pack, "run-1");
    assert!(
        summary.contains("plain=1") && summary.contains("secret=0"),
        "{summary}"
    );
}

#[test]
fn reset_between_runs_leaves_no_prior_run_state_in_the_same_process() {
    let pack = Pack::connect("fake", spawn(&[]), timeouts(), env()).unwrap();
    pack.call(
        "run-1",
        None,
        "fake_regions",
        json!({ "image": "candidate", "_squint": context("run-1") }),
    )
    .unwrap();
    let (calls, pid_a, _) = probe(&pack, "run-1");
    assert_eq!(calls, 2, "the fake call and this probe");
    let (calls, pid_b, _) = probe(&pack, "run-2");
    assert_eq!(calls, 1, "run-2 starts from nothing");
    assert_eq!(pid_a, pid_b, "reset, not restart");
    assert_eq!(pack.restarts(), 0);
}

#[test]
fn a_server_without_reset_is_restarted_per_run() {
    let pack = Pack::connect("fake", spawn(&["--no-reset"]), timeouts(), env()).unwrap();
    let (_, pid_a, _) = probe(&pack, "run-1");
    assert_eq!(
        pack.restarts(),
        0,
        "the first run takes the fresh server as is"
    );
    let (calls, pid_b, _) = probe(&pack, "run-2");
    assert_ne!(pid_a, pid_b, "a fresh process");
    assert_eq!(calls, 1);
    assert_eq!(pack.restarts(), 1);
}

#[test]
fn a_timed_out_call_is_cancelled_tombstoned_and_the_process_never_reused() {
    let mut t = timeouts();
    t.call = Duration::from_millis(300);
    let pack = Pack::connect("fake", spawn(&["--slow-call-ms", "1500"]), t, env()).unwrap();
    let pid_a = u64::from(pack.pid().unwrap());
    let err = pack
        .call(
            "run-1",
            None,
            "fake_regions",
            json!({ "image": "candidate", "_squint": context("run-1") }),
        )
        .err()
        .unwrap();
    assert_eq!(err.class(), "tool");
    assert!(err.to_string().contains("timed out"), "{err}");
    // The next call, even in the same run, replaces the process: the late answer is never read.
    let (calls, pid_b, _) = probe(&pack, "run-1");
    assert_ne!(pid_a, pid_b);
    assert_eq!(calls, 1);
    assert_eq!(pack.restarts(), 1);
}

#[test]
fn the_acquisition_deadline_is_one_clock_covering_a_hanging_reset() {
    let mut t = timeouts();
    t.startup = Duration::from_millis(600);
    let pack = Pack::connect("fake", spawn(&["--hang-reset"]), t, env()).unwrap();
    probe(&pack, "run-1");
    let started = Instant::now();
    let err = pack
        .call(
            "run-2",
            None,
            "probe_state",
            json!({ "_squint": context("run-2") }),
        )
        .err()
        .unwrap();
    let elapsed = started.elapsed();
    assert!(err.to_string().contains("deadline"), "{err}");
    assert!(
        elapsed < Duration::from_millis(1500),
        "one deadline, not one per step: {elapsed:?}"
    );
    // A later acquisition starts a fresh clock and restarts the server.
    let (calls, _, _) = probe(&pack, "run-3");
    assert_eq!(calls, 1);
    assert!(pack.restarts() >= 1);
}

#[test]
fn shutdown_escalates_past_a_server_that_ignores_stdin_close() {
    let client = Client::connect(
        &spawn(&["--ignore-stdin-close"]),
        env(),
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    let pid = client.pid();
    let started = Instant::now();
    client.shutdown(Duration::from_millis(300));
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_millis(300),
        "stdin close alone did not end it"
    );
    assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    #[cfg(unix)]
    {
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the process is gone");
    }
}
