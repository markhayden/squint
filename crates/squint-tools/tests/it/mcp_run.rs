//! Task 4.4a through the run: a batch of calls to pack tools that declare `concurrent = true`
//! starts together and settles in call order; without the declaration the same batch runs one
//! call after another. A rejected call in the batch and a cancel during it behave as they do for
//! built-ins, and a pack whose server dies is restarted under the next lease.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::run;
use squint_core::tool::ToolRegistry;
use squint_testkit::{ScriptedProvider, mcp_server};
use squint_tools::mcp::{Pack, Spawn, Timeouts};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-mcp-run-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn submission(items: &[(&str, &[&str])]) -> serde_json::Value {
    let items: Vec<serde_json::Value> = items
        .iter()
        .map(|(id, evidence)| {
            serde_json::json!({ "id": id, "disposition": "held", "confidence": "confident", "reason": "scripted", "allowed_by": null, "location": null, "evidence": evidence })
        })
        .collect();
    serde_json::json!({
        "tool_calls": [{ "id": "c_submit", "name": "submit_verdict", "arguments": {
            "schema_version": 1,
            "items": items,
            "findings": [],
            "opinion": { "verdict": "pass", "reason": "r" }
        } }],
        "stop_reason": "tool_use"
    })
}

/// Two scripted region calls in one batch, then a submission citing both.
fn script() -> serde_json::Value {
    serde_json::json!([
        {
            "tool_calls": [
                { "id": "c1", "name": "fake_regions", "arguments": { "image": "candidate" } },
                { "id": "c2", "name": "fake_regions", "arguments": { "image": "candidate" } }
            ],
            "stop_reason": "tool_use"
        },
        submission(&[
            ("chk:ob:SF1@candidate", &["call:1", "call:2"]),
            ("chk:ob:SF2@candidate", &[]),
            ("chk:ev:1:0", &[]),
            ("chk:ev:2:0", &[]),
        ])
    ])
}

/// The first call names an image the job does not have; the second is fine.
fn script_with_a_rejected_call() -> serde_json::Value {
    serde_json::json!([
        {
            "tool_calls": [
                { "id": "c1", "name": "fake_regions", "arguments": { "image": "nope" } },
                { "id": "c2", "name": "fake_regions", "arguments": { "image": "candidate" } }
            ],
            "stop_reason": "tool_use"
        },
        submission(&[
            ("chk:ob:SF1@candidate", &["call:2"]),
            ("chk:ob:SF2@candidate", &[]),
            ("chk:ev:2:0", &[]),
        ])
    ])
}

fn connect(flags: &[&str], slow_ms: u64) -> Arc<Pack> {
    let mut args: Vec<String> = vec![
        "--envelopes".into(),
        // One region per envelope, so the evidence items are the same whichever call the server
        // happens to serve first.
        root()
            .join("fixtures/tool-scripts/regions-1-then-1.json")
            .display()
            .to_string(),
        "--slow-call-ms".into(),
        slow_ms.to_string(),
    ];
    args.extend(flags.iter().map(|s| s.to_string()));
    Pack::connect(
        "fake",
        Spawn {
            command: mcp_server::path(),
            args,
            declared_env: vec![],
            cwd: None,
            extra_env: vec![],
        },
        Timeouts {
            startup: Duration::from_secs(10),
            call: Duration::from_secs(5),
            shutdown_grace: Duration::from_millis(300),
        },
        std::env::vars(),
    )
    .unwrap()
}

struct Outcome {
    elapsed: Duration,
    exit_code: i32,
    error: Option<String>,
    report: serde_json::Value,
}

/// Runs one decision against the fake pack; `cancel_after` fires the harness's cancel then.
fn through_the_run(
    pack: &Arc<Pack>,
    script: serde_json::Value,
    tag: &str,
    cancel_after: Option<Duration>,
) -> Outcome {
    let out = tempdir(tag);
    let registry = ToolRegistry::new(pack.tools(), &["fake_regions".into()], &[]).unwrap();
    let provider = ScriptedProvider::new(serde_json::from_value(script).unwrap());
    let config = Config::parse(
        &std::fs::read_to_string(root().join("fixtures/config/fake-tool.toml")).unwrap(),
        &["budget.spend_cap_usd=100".into()],
        &|_| None,
    )
    .unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let cancel = run::Cancel::new();
    let base = root();
    let started = Instant::now();
    let o = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let services = run::Services::new(&provider, &registry).with_cancel(&cancel);
            let check = run::check(&config, &job, &base, &services, &out, "test");
            let trigger = async {
                if let Some(after) = cancel_after {
                    tokio::time::sleep(after).await;
                    cancel.cancel();
                }
            };
            let (o, ()) = tokio::join!(check, trigger);
            o.unwrap()
        });
    let elapsed = started.elapsed();
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    Outcome {
        elapsed,
        exit_code: o.exit_code,
        error: o.error,
        report,
    }
}

#[test]
fn a_batch_of_concurrent_pack_tools_runs_together_and_settles_in_call_order() {
    let pack = connect(&["--concurrent"], 400);
    let o = through_the_run(&pack, script(), "together", None);
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    assert!(
        o.elapsed < Duration::from_millis(1500),
        "two 400 ms calls together, not in turn (plus the run's own work): {:?}",
        o.elapsed
    );
    let calls = o.report["runs"][0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        (calls[0]["id"].as_str(), calls[1]["id"].as_str()),
        (Some("call:1"), Some("call:2"))
    );
    // One region from each scripted envelope, in call order.
    assert_eq!(calls[0]["added_items"].as_array().unwrap().len(), 1);
    assert_eq!(calls[1]["added_items"].as_array().unwrap().len(), 1);
    let items: Vec<&str> = o.report["runs"][0]["checklist"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["id"].as_str())
        .filter(|id| id.starts_with("chk:ev:"))
        .collect();
    assert_eq!(items, ["chk:ev:1:0", "chk:ev:2:0"]);
}

#[test]
fn a_batch_of_serial_pack_tools_runs_one_call_after_another() {
    let pack = connect(&[], 400);
    let o = through_the_run(&pack, script(), "in-turn", None);
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    assert!(o.elapsed >= Duration::from_millis(800), "{:?}", o.elapsed);
    assert_eq!(
        o.report["runs"][0]["tool_calls"].as_array().unwrap().len(),
        2
    );
}

#[test]
fn a_rejected_call_in_a_concurrent_batch_is_answered_while_the_rest_run() {
    let pack = connect(&["--concurrent"], 0);
    let o = through_the_run(&pack, script_with_a_rejected_call(), "rejected", None);
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let calls = o.report["runs"][0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0]["id"], "call:1");
    assert_eq!(calls[0]["executed"], false);
    assert!(
        calls[0]["invalid_args"]
            .as_str()
            .unwrap()
            .contains("unknown image id"),
        "{}",
        calls[0]
    );
    assert_eq!(calls[1]["id"], "call:2");
    assert_eq!(calls[1]["executed"], true);
    assert_eq!(calls[1]["status"], "ok");
    assert_eq!(calls[1]["added_items"].as_array().unwrap().len(), 1);
    let items: Vec<&str> = o.report["runs"][0]["checklist"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|i| i["id"].as_str())
        .filter(|id| id.starts_with("chk:ev:"))
        .collect();
    assert_eq!(items, ["chk:ev:2:0"]);
}

#[test]
fn a_cancel_during_a_concurrent_batch_lands_without_waiting_for_the_calls() {
    let pack = connect(&["--concurrent"], 3000);
    let o = through_the_run(&pack, script(), "cancel", Some(Duration::from_millis(150)));
    assert!(
        o.elapsed < Duration::from_millis(2500),
        "two 3 s calls did not block the cancel: {:?}",
        o.elapsed
    );
    assert_eq!(o.exit_code, 3);
    assert_eq!(o.report["runs"][0]["error"]["class"], "cancelled");
}

/// A server that exits after every call: the next run's lease finds it gone and restarts it, and
/// so does a later call of the same run.
#[test]
fn a_pack_whose_server_died_is_restarted_under_the_next_lease_and_within_a_run() {
    let pack = connect(&["--exit-after-calls", "1"], 0);
    let dir_a = tempdir("died-a");
    let dir_b = tempdir("died-b");
    let args = |n: u32| serde_json::json!({ "image": "candidate", "_squint": { "run_id": "run", "call_id": format!("call:{n}") } });
    let first = pack
        .call("run", Some(&dir_a), "fake_regions", args(1))
        .unwrap();
    assert_eq!(
        first.status,
        squint_core::tool::Status::Ok,
        "{}",
        first.summary
    );
    assert_eq!(pack.restarts(), 0);
    // The server exits right after answering; the reader notices the end of its output a moment
    // later, as it would between two decisions. A server that dies in the middle of a call is
    // that call's error, not a restart.
    let gone = || std::thread::sleep(Duration::from_millis(300));
    gone();
    // Another decision's run-1: a new lease over a dead server.
    let second = pack
        .call("run", Some(&dir_b), "fake_regions", args(1))
        .unwrap();
    assert_eq!(
        second.status,
        squint_core::tool::Status::Ok,
        "{}",
        second.summary
    );
    assert_eq!(pack.restarts(), 1);
    gone();
    // The same run's next call: the lease holder's server is gone too.
    let third = pack
        .call("run", Some(&dir_b), "fake_regions", args(2))
        .unwrap();
    // The script holds two envelopes per server, and a restarted server starts the script over.
    assert_eq!(
        third.status,
        squint_core::tool::Status::Ok,
        "{}",
        third.summary
    );
    assert_eq!(pack.restarts(), 2);
}
