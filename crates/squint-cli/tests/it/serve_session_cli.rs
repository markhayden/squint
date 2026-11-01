//! Task 8.2: the serving session (table-manners 4.6): a random UUID per server, the
//! `squint://session` resource read without waiting on a running check, its reservation moving
//! with the check, and every run the server makes carrying the session's id.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::json;
use squint_testkit::mcp_client::Client;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn out(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("squint-serve-session-{tag}-{}", std::process::id()));
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

#[test]
fn the_session_is_a_listed_resource_with_a_fresh_uuid_per_server() {
    let mut a = client(&out("uuid-a"), "held.json");
    let mut b = client(&out("uuid-b"), "held.json");
    let first = session(&mut a);
    let second = session(&mut b);
    assert_ne!(first["id"], second["id"]);
    assert_eq!(first["budget_usd"], 20.0);
    assert_eq!(first["spent_usd"], 0.0);
    assert_eq!(first["remaining_usd"], 20.0);
    let listed = a.request("resources/list", json!({})).unwrap();
    assert!(
        listed["resources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["uri"] == "squint://session"),
        "{listed}"
    );
}

#[test]
fn every_run_a_server_makes_carries_its_session() {
    let mut c = client(&out("runs"), "held.json");
    let id = session(&mut c)["id"].clone();
    let checked = c
        .call("source_fidelity", pair())
        .unwrap()
        .structured
        .unwrap();
    assert_eq!(checked["run"]["session_id"], id);
    let mut call = pair();
    call["use_case"] = json!("source-fidelity");
    let estimated = c.call("estimate", call).unwrap();
    let estimated = estimated.structured.unwrap();
    assert_eq!(estimated["run"]["session_id"], id);
    assert!(
        estimated["body"].get("session_budget_left_usd").is_none(),
        "{estimated}"
    );
}

#[test]
fn the_session_answers_at_once_while_a_check_holds_its_reservation() {
    let mut c = client(&out("busy"), "slow-retry.json");
    let first = c.start_call("source_fidelity", pair(), "tok").unwrap();
    // The retry wait's heartbeat: by then the decision has reserved its worst case.
    loop {
        let n = c.next_notification().unwrap();
        if n["params"]["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("waiting"))
        {
            break;
        }
    }
    let asked = Instant::now();
    let busy = session(&mut c);
    assert!(asked.elapsed() < Duration::from_secs(3));
    assert!(busy["reserved_usd"].as_f64().unwrap() > 0.0, "{busy}");
    c.cancel(first).unwrap();
    let settled = Instant::now();
    loop {
        let now = session(&mut c);
        if now["reserved_usd"] == 0.0 {
            break;
        }
        assert!(settled.elapsed() < Duration::from_secs(10), "{now}");
        std::thread::sleep(Duration::from_millis(100));
    }
}
