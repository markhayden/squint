//! Task 7.8: long runs over `squint serve` (table-manners 4): progress strictly increasing and
//! naming the run first, heartbeats while a call waits, cancellation with no result and a
//! readable `cancelled` run, the runs as resources, and a repeated call answered from the cache.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::json;
use squint_testkit::mcp_client::{Client, Failure};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn out(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "squint-serve-progress-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn client(out: &Path, script: &str, overrides: &[&str]) -> Client {
    let mut c = Command::new(env!("CARGO_BIN_EXE_squint"));
    c.current_dir(repo_root())
        .args([
            "serve",
            "--config",
            "fixtures/config/minimal.toml",
            "--provider-script",
        ])
        .arg(repo_root().join("fixtures/scripts").join(script))
        .arg("--out")
        .arg(out);
    for o in overrides {
        c.args(["--set", o]);
    }
    let mut client = Client::start(&mut c).unwrap();
    client.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/images").display()
    )];
    client
}

fn pair() -> serde_json::Value {
    let image = |n: &str| json!({ "path": repo_root().join("fixtures/images").join(n) });
    json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") })
}

fn progress(c: &Client, token: &str) -> Vec<serde_json::Value> {
    c.notifications
        .iter()
        .filter(|n| {
            n["method"] == "notifications/progress" && n["params"]["progressToken"] == token
        })
        .map(|n| n["params"].clone())
        .collect()
}

fn increasing(values: &[serde_json::Value]) {
    for w in values.windows(2) {
        assert!(
            w[1]["progress"].as_f64().unwrap() > w[0]["progress"].as_f64().unwrap(),
            "progress must increase: {w:?}"
        );
    }
}

#[test]
fn progress_names_the_run_first_and_increases_on_every_step() {
    let mut c = client(&out("steps"), "held.json", &[]);
    let r = c
        .call_with_progress("source_fidelity", pair(), "tok-1")
        .unwrap();
    let run = r.structured.unwrap();
    let p = progress(&c, "tok-1");
    assert!(!p.is_empty());
    assert!(
        p[0]["message"]
            .as_str()
            .unwrap()
            .contains(run["run"]["resource"].as_str().unwrap()),
        "{:?}",
        p[0]
    );
    increasing(&p);
    assert!(p.iter().all(|n| n.get("total").is_none()));
    let journal = std::fs::read_to_string(
        PathBuf::from(run["run"]["dir"].as_str().unwrap()).join("run-1/journal.jsonl"),
    )
    .unwrap();
    assert!(
        p.len() >= journal.lines().count(),
        "one notification per step at least"
    );
}

/// A call waiting on a provider's retry sends heartbeats; a cancel stops it, no result comes, and
/// the run is readable as `cancelled` through the resources.
#[test]
fn a_waiting_call_beats_and_a_cancel_leaves_a_cancelled_run_and_no_result() {
    let mut c = client(&out("cancel"), "slow-retry.json", &[]);
    let id = c.start_call("source_fidelity", pair(), "tok-2").unwrap();
    let started = Instant::now();
    loop {
        let n = c.next_notification().unwrap();
        if n["params"]["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("waiting"))
        {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "no heartbeat");
    }
    // The next beat comes within the declared interval (table-manners 4.1: 10 s).
    let beat = Instant::now();
    c.next_notification().unwrap();
    assert!(
        beat.elapsed() <= Duration::from_secs(10),
        "a {:?} gap between heartbeats",
        beat.elapsed()
    );
    increasing(&progress(&c, "tok-2"));
    // While it waits, the run is listed as running under its action, as `describe` names it.
    let runs: serde_json::Value =
        serde_json::from_str(&c.read_resource("squint://runs").unwrap()).unwrap();
    let running = &runs["runs"][0];
    assert_eq!(running["status"], "running", "{runs}");
    assert_eq!(running["action"], "source_fidelity", "{runs}");
    c.cancel(id).unwrap();
    let mut cancelled = None;
    for _ in 0..100 {
        let runs: serde_json::Value =
            serde_json::from_str(&c.read_resource("squint://runs").unwrap()).unwrap();
        if let Some(r) = runs["runs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["status"] == "cancelled")
        {
            cancelled = Some(r.clone());
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let run = cancelled.expect("the run became cancelled");
    let result: serde_json::Value =
        serde_json::from_str(&c.read_resource(run["resource"].as_str().unwrap()).unwrap()).unwrap();
    assert_eq!(result["status"], "cancelled");
    c.call("describe", json!({})).unwrap();
    assert!(!c.has_response(id), "a cancelled request gets no response");
}

#[test]
fn runs_are_resources_and_an_unknown_run_is_an_error() {
    let mut c = client(&out("resources"), "held.json", &[]);
    let r = c
        .call("source_fidelity", pair())
        .unwrap()
        .structured
        .unwrap();
    let listed = c.request("resources/list", json!({})).unwrap();
    assert!(
        listed["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["uri"] == "squint://runs")
    );
    let runs: serde_json::Value =
        serde_json::from_str(&c.read_resource("squint://runs").unwrap()).unwrap();
    assert_eq!(runs["tool"]["name"], "squint");
    assert_eq!(runs["runs"][0]["id"], r["run"]["id"]);
    assert_eq!(runs["runs"][0]["status"], "completed");
    let id = r["run"]["id"].as_str().unwrap();
    let on_resource: serde_json::Value = serde_json::from_str(
        &c.read_resource(&format!("squint://run/{id}/run.json"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(on_resource, r);
    let journal = c
        .read_resource(&format!("squint://run/{id}/journal"))
        .unwrap();
    assert!(journal.lines().count() > 1);
    let report = c
        .read_resource(&format!("squint://run/{id}/report.json"))
        .unwrap();
    assert!(report.contains("\"verdict\""));
    for bad in [
        "squint://run/no-such-run/run.json",
        "squint://run/../run.json",
    ] {
        assert!(
            matches!(
                c.request("resources/read", json!({ "uri": bad })),
                Err(Failure::Rpc { .. })
            ),
            "{bad}"
        );
    }
}

#[test]
fn a_repeated_call_is_answered_from_the_cache_and_costs_nothing() {
    let dir = out("cache");
    let cache = dir.join("cache");
    let set = format!("cache.dir={}", cache.display());
    let mut c = client(&dir, "held.json", &[set.as_str()]);
    let first = c
        .call("source_fidelity", pair())
        .unwrap()
        .structured
        .unwrap();
    assert_eq!(first["cache"], "miss");
    let again = c
        .call("source_fidelity", pair())
        .unwrap()
        .structured
        .unwrap();
    assert_eq!(again["cache"], "hit");
    assert_eq!(again["spend"]["usd"], 0.0);
    assert_eq!(again["body"]["verdict"], first["body"]["verdict"]);
}

/// While a check holds the only permit (table-manners C4 review 9, 10; M7 review I4): `describe`
/// answers at once, a second check waiting for the permit beats within the interval, and
/// cancelling it while it waits leaves no run.
#[test]
fn a_queued_call_beats_and_cancels_without_a_run_while_describe_answers() {
    let mut c = client(&out("queued"), "slow-retry.json", &[]);
    let first = c.start_call("source_fidelity", pair(), "tok-a").unwrap();
    // The first check is running once it reports progress.
    loop {
        let n = c.next_notification().unwrap();
        if n["params"]["progressToken"] == "tok-a" {
            break;
        }
    }
    let asked = Instant::now();
    c.call("describe", json!({})).unwrap();
    assert!(
        asked.elapsed() < Duration::from_secs(3),
        "describe waited {:?} behind a running check",
        asked.elapsed()
    );

    let queued = c.start_call("source_fidelity", pair(), "tok-b").unwrap();
    let waiting = Instant::now();
    loop {
        let n = c.next_notification().unwrap();
        if n["params"]["progressToken"] == "tok-b" {
            break;
        }
        assert!(
            waiting.elapsed() <= Duration::from_secs(10),
            "a queued call went silent"
        );
    }
    c.cancel(queued).unwrap();
    // The cancel lands while it waits: it never gets a run, and the first check is the only one.
    std::thread::sleep(Duration::from_millis(500));
    let runs: serde_json::Value =
        serde_json::from_str(&c.read_resource("squint://runs").unwrap()).unwrap();
    assert_eq!(runs["runs"].as_array().unwrap().len(), 1, "{runs}");
    c.cancel(first).unwrap();
}
