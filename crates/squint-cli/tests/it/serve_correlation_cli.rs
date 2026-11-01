//! Task 8.3: correlation and the run reference (table-manners 4.3, 4.4): a client's
//! `client_run_id` claimed when its request is accepted and refused when reused, and the run's
//! reference on every progress notification from allocation and in every listing. Task 9.7: a
//! request that ends in a JSON-RPC error with no run releases its id; one cancelled before
//! allocation keeps it.
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
        "squint-serve-correlation-{tag}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn client(out: &Path, script: &str) -> Client {
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

/// A document valid against one of the contract's schemas, by id.
fn valid(id: &str, doc: &serde_json::Value) {
    struct Embedded;
    impl jsonschema::Retrieve for Embedded {
        fn retrieve(
            &self,
            uri: &jsonschema::Uri<String>,
        ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
            let (_, text) = table_manners::SCHEMAS
                .iter()
                .find(|(id, _)| *id == uri.as_str())
                .ok_or("unknown schema")?;
            Ok(serde_json::from_str(text)?)
        }
    }
    let (_, text) = table_manners::SCHEMAS
        .iter()
        .find(|(schema, _)| *schema == id)
        .unwrap();
    let schema: serde_json::Value = serde_json::from_str(text).unwrap();
    let validator = jsonschema::options()
        .with_retriever(Embedded)
        .build(&schema)
        .unwrap();
    let errors: Vec<String> = validator.iter_errors(doc).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{id}: {errors:?}\n{doc}");
}

fn session(c: &mut Client) -> serde_json::Value {
    let doc: serde_json::Value =
        serde_json::from_str(&c.read_resource("squint://session").unwrap()).unwrap();
    valid("urn:table-manners:core:session", &doc);
    doc
}

const CLIENT: &str = "table-manners/client_run_id";
const RUN: &str = "table-manners/run";

/// The JSON-RPC error a call failed with: its code and data.
fn refused(r: Result<squint_testkit::mcp_client::CallResult, Failure>) -> (i64, serde_json::Value) {
    match r {
        Err(Failure::Rpc { code, data, .. }) => (code, data.unwrap_or_default()),
        other => panic!("expected a JSON-RPC error, got {other:?}"),
    }
}

/// The first progress notification for `token`. Progress starts only after the request is
/// accepted, and so after its id is claimed: the claim is in place once this returns. Two
/// requests in flight together have no guaranteed order, so a test that needs one accepted
/// before the next is sent waits here first.
fn first_progress(c: &mut Client, token: &str) -> serde_json::Value {
    loop {
        let n = c.next_notification().unwrap();
        if n["params"]["progressToken"] == token {
            return n;
        }
    }
}

#[test]
fn an_accepted_id_is_the_runs_and_a_reused_one_is_refused_with_that_run() {
    let mut c = client(&out("reuse"), "held.json");
    let session_id = session(&mut c)["id"].clone();
    let first = c
        .call_with_meta(
            "source_fidelity",
            pair(),
            json!({ CLIENT: "host-attempt-1" }),
        )
        .unwrap()
        .structured
        .unwrap();
    assert_eq!(first["run"]["client_run_id"], "host-attempt-1");
    assert_eq!(first["run"]["session_id"], session_id);
    let runs: serde_json::Value =
        serde_json::from_str(&c.read_resource("squint://runs").unwrap()).unwrap();
    valid("urn:table-manners:core:runs", &runs);
    assert_eq!(runs["runs"][0]["client_run_id"], "host-attempt-1");

    let (code, data) = refused(c.call_with_meta(
        "source_fidelity",
        pair(),
        json!({ CLIENT: "host-attempt-1" }),
    ));
    assert_eq!(code, -32602);
    valid("urn:table-manners:core:refusal", &data);
    assert_eq!(data["reason"], "client_run_id_reused");
    assert_eq!(data["run"]["id"], first["run"]["id"]);
    assert_eq!(data["run"]["resource"], first["run"]["resource"]);
    assert_eq!(data["run"]["session_id"], session_id);
    assert_eq!(data["run"]["client_run_id"], "host-attempt-1");
    // An estimate claims ids the same way.
    let mut call = pair();
    call["use_case"] = json!("source-fidelity");
    let (code, _) =
        refused(c.call_with_meta("estimate", call, json!({ CLIENT: "host-attempt-1" })));
    assert_eq!(code, -32602);
}

#[test]
fn a_malformed_id_is_invalid_params() {
    let mut c = client(&out("malformed"), "held.json");
    for bad in [
        json!("has space"),
        json!(""),
        json!("x".repeat(129)),
        json!(5),
        json!(["a"]),
    ] {
        let (code, data) =
            refused(c.call_with_meta("source_fidelity", pair(), json!({ CLIENT: bad.clone() })));
        assert_eq!(code, -32602, "{bad}");
        assert!(data.get("reason").is_none(), "{bad}: not a reuse: {data}");
    }
}

/// The runs list once it holds `count` runs and none is still running.
fn runs_once_ended(c: &mut Client, count: usize) -> serde_json::Value {
    let started = Instant::now();
    loop {
        let runs: serde_json::Value =
            serde_json::from_str(&c.read_resource("squint://runs").unwrap()).unwrap();
        let entries = runs["runs"].as_array().unwrap();
        if entries.len() == count && entries.iter().all(|r| r["status"] != "running") {
            return runs;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "runs never ended: {runs}"
        );
    }
}

/// A request refused with a JSON-RPC error before any run releases its id (table-manners 4.4,
/// squint's M8 review S3): the client fixes the request and sends it again under the same id,
/// which is accepted, and its run carries the id. The refusals left no run behind.
#[test]
fn an_id_refused_with_no_run_is_released_for_the_corrected_request() {
    let mut c = client(&out("released"), "held.json");
    let mut unknown = pair();
    unknown["colour"] = json!("blue");
    let mut no_cap = pair();
    no_cap["max_spend_usd"] = json!("a dollar");
    for (tool, call) in [
        ("source_fidelity", unknown),
        ("source_fidelity", no_cap),
        // An estimate that names no use case.
        ("estimate", pair()),
    ] {
        let (code, data) = refused(c.call_with_meta(tool, call, json!({ CLIENT: "fix-1" })));
        assert_eq!(code, -32602, "{tool}");
        assert!(data.get("reason").is_none(), "{tool}: not a reuse: {data}");
    }
    let runs: serde_json::Value =
        serde_json::from_str(&c.read_resource("squint://runs").unwrap()).unwrap();
    assert_eq!(runs["runs"], json!([]), "a refusal makes no run");

    let accepted = c
        .call_with_meta("source_fidelity", pair(), json!({ CLIENT: "fix-1" }))
        .unwrap()
        .structured
        .unwrap();
    assert_eq!(accepted["status"], "completed", "{accepted}");
    assert_eq!(accepted["run"]["client_run_id"], "fix-1");
    // Once it has a run, the id is that run's for the session.
    let (code, data) =
        refused(c.call_with_meta("source_fidelity", pair(), json!({ CLIENT: "fix-1" })));
    assert_eq!(code, -32602);
    valid("urn:table-manners:core:refusal", &data);
    assert_eq!(data["run"]["id"], accepted["run"]["id"]);
}

/// An id is claimed when its request is accepted, so one cancelled while it waited for the
/// permit stays claimed: the next request with it is refused, with no run to point at.
#[test]
fn an_id_cancelled_before_its_run_stays_claimed_with_no_run() {
    let mut c = client(&out("queued"), "slow-retry.json");
    let running = c
        .start_call_with_meta(
            "source_fidelity",
            pair(),
            json!({ "progressToken": "a", CLIENT: "a" }),
        )
        .unwrap();
    first_progress(&mut c, "a");
    let queued = c
        .start_call_with_meta(
            "source_fidelity",
            pair(),
            json!({ "progressToken": "q", CLIENT: "q" }),
        )
        .unwrap();
    // Its heartbeat: accepted, claimed and waiting for the permit, so the cancel lands there.
    first_progress(&mut c, "q");
    c.cancel(queued).unwrap();
    // The server reads the two cancellations in order. Once the running call's run has ended,
    // the permit is free, and the queued call, cancelled first, has given up its wait with no
    // run: the one run listed is the running call's.
    c.cancel(running).unwrap();
    let runs = runs_once_ended(&mut c, 1);
    assert_eq!(runs["runs"][0]["client_run_id"], "a", "{runs}");
    let mut call = pair();
    call["use_case"] = json!("source-fidelity");
    let (code, data) = refused(c.call_with_meta("estimate", call, json!({ CLIENT: "q" })));
    assert_eq!(code, -32602);
    valid("urn:table-manners:core:refusal", &data);
    assert_eq!(data["reason"], "client_run_id_reused");
    assert!(data["run"].is_null(), "{data}");
}

/// Two requests sent together with one id: the first accepted takes it, the other is refused.
#[test]
fn concurrent_requests_with_one_id_run_once() {
    let mut c = client(&out("concurrent"), "held.json");
    let mut call = pair();
    call["use_case"] = json!("source-fidelity");
    let one = c
        .start_call_with_meta("estimate", call.clone(), json!({ CLIENT: "same" }))
        .unwrap();
    let two = c
        .start_call_with_meta("estimate", call, json!({ CLIENT: "same" }))
        .unwrap();
    let results = [c.finish(one), c.finish(two)];
    let accepted = results.iter().filter(|r| r.is_ok()).count();
    assert_eq!(accepted, 1, "{results:?}");
    let refusal = results.into_iter().find(|r| r.is_err()).unwrap();
    let (code, data) = refused(refusal);
    assert_eq!(code, -32602);
    valid("urn:table-manners:core:refusal", &data);
    assert_eq!(data["reason"], "client_run_id_reused");
}

/// An id is refused while its request still waits for the permit (no run yet) and while its run
/// is going (that run); a queued call's heartbeats carry no run reference until it has one.
#[test]
fn an_id_in_use_is_refused_while_queued_and_while_running() {
    let mut c = client(&out("in-use"), "slow-retry.json");
    let running = c
        .start_call_with_meta(
            "source_fidelity",
            pair(),
            json!({ "progressToken": "a", CLIENT: "a" }),
        )
        .unwrap();
    let reference = first_progress(&mut c, "a")["params"]["_meta"][RUN].clone();
    let queued = c
        .start_call_with_meta(
            "source_fidelity",
            pair(),
            json!({ "progressToken": "q", CLIENT: "q" }),
        )
        .unwrap();
    // The queued call's heartbeat, before any allocation: no reference. It also shows "q" is
    // accepted and claimed; sent without waiting for it, the estimate below could be accepted
    // first and take the id itself.
    let heartbeat = first_progress(&mut c, "q");
    assert!(
        heartbeat["params"]["message"]
            .as_str()
            .unwrap()
            .starts_with("waiting"),
        "{heartbeat}"
    );
    assert!(
        heartbeat["params"]["_meta"].get(RUN).is_none(),
        "{heartbeat}"
    );
    let mut call = pair();
    call["use_case"] = json!("source-fidelity");
    let (code, data) = refused(c.call_with_meta("estimate", call.clone(), json!({ CLIENT: "q" })));
    assert_eq!(code, -32602);
    valid("urn:table-manners:core:refusal", &data);
    assert!(data["run"].is_null(), "queued, so no run yet: {data}");
    let (code, data) = refused(c.call_with_meta("estimate", call, json!({ CLIENT: "a" })));
    assert_eq!(code, -32602);
    assert_eq!(data["run"], reference, "running, so its run");
    c.cancel(queued).unwrap();
    c.cancel(running).unwrap();
}

/// From allocation, every progress notification names the run by reference, heartbeats too, and
/// the message no longer carries its address; a running entry carries the same pair.
#[test]
fn progress_carries_the_run_reference_from_allocation() {
    let mut c = client(&out("meta"), "slow-retry.json");
    let session_id = session(&mut c)["id"].clone();
    let id = c
        .start_call_with_meta(
            "source_fidelity",
            pair(),
            json!({ "progressToken": "t", CLIENT: "host-7" }),
        )
        .unwrap();
    let started = Instant::now();
    let mut seen = Vec::new();
    loop {
        let n = c.next_notification().unwrap();
        if n["params"]["progressToken"] != "t" {
            continue;
        }
        let waiting = n["params"]["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("waiting"));
        seen.push(n["params"].clone());
        if waiting {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(20));
    }
    let reference = seen[0]["_meta"][RUN].clone();
    valid("urn:table-manners:core:run-ref", &reference);
    assert_eq!(reference["session_id"], session_id);
    assert_eq!(reference["client_run_id"], "host-7");
    for params in &seen {
        assert_eq!(params["_meta"][RUN], reference, "{params}");
        assert!(
            !params["message"].as_str().unwrap().contains("squint://"),
            "{params}"
        );
    }
    let runs: serde_json::Value =
        serde_json::from_str(&c.read_resource("squint://runs").unwrap()).unwrap();
    let running = &runs["runs"][0];
    assert_eq!(running["status"], "running");
    assert_eq!(running["id"], reference["id"]);
    assert_eq!(running["client_run_id"], "host-7");
    c.cancel(id).unwrap();
}
