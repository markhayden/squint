//! Task 8.3: correlation and the run reference (table-manners 4.3, 4.4): a client's
//! `client_run_id` claimed when its request is accepted and refused when reused, and the run's
//! reference on every progress notification from allocation and in every listing.
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
    for bad in ["has space", "", &"x".repeat(129)] {
        let (code, data) =
            refused(c.call_with_meta("source_fidelity", pair(), json!({ CLIENT: bad })));
        assert_eq!(code, -32602, "{bad:?}");
        assert!(data.get("reason").is_none(), "{bad:?}: not a reuse: {data}");
    }
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
    loop {
        if c.next_notification().unwrap()["params"]["progressToken"] == "a" {
            break;
        }
    }
    let queued = c
        .start_call_with_meta(
            "source_fidelity",
            pair(),
            json!({ "progressToken": "q", CLIENT: "q" }),
        )
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    c.cancel(queued).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let mut call = pair();
    call["use_case"] = json!("source-fidelity");
    let (code, data) = refused(c.call_with_meta("estimate", call, json!({ CLIENT: "q" })));
    assert_eq!(code, -32602);
    assert_eq!(data["reason"], "client_run_id_reused");
    assert!(data["run"].is_null(), "{data}");
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
