//! Task 8.2: the serving session (table-manners 4.6): a random UUID per server, the
//! `squint://session` resource read without waiting on a running check, its reservation moving
//! with the check, and every run the server makes carrying the session's id. Task 9.3: readiness
//! from the session's provider answers, and the proof that describing and estimating call no
//! provider.
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
    // An estimate is no run, but it is bound to the session and observes it (table-manners 4.5).
    let estimated = c
        .call(
            "estimate",
            json!({ "action": "source_fidelity", "arguments": pair() }),
        )
        .unwrap()
        .structured
        .unwrap();
    valid("urn:table-manners:core:estimate", &estimated);
    assert_eq!(estimated["session_id"], id);
    assert_eq!(estimated["session"]["id"], id);
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
            // The failed attempt was sent and settled by no answer: all the spend is unresolved.
            let spent = now["spent_usd"].as_f64().unwrap();
            assert!(spent > 0.0, "{now}");
            // Every amount to at most six decimals (table-manners 4.6).
            for key in [
                "budget_usd",
                "spent_usd",
                "unresolved_usd",
                "reserved_usd",
                "remaining_usd",
            ] {
                let text = now[key].to_string();
                assert!(
                    text.split('.').nth(1).is_none_or(|d| d.len() <= 6),
                    "{key}: {text}"
                );
            }
            assert!(
                (now["unresolved_usd"].as_f64().unwrap() - spent).abs() < 1e-9,
                "{now}"
            );
            break;
        }
        assert!(settled.elapsed() < Duration::from_secs(10), "{now}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The credential and model of every action, as the next `describe` reports them.
fn credential_and_model(c: &mut Client) -> (serde_json::Value, serde_json::Value) {
    let d = c.call("describe", json!({})).unwrap().structured.unwrap();
    valid("urn:table-manners:core:description", &d);
    let requirements = d["actions"][0]["readiness"]["requirements"].clone();
    let state = |kind: &str| {
        requirements
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == kind)
            .map(|r| r["state"].clone())
            .unwrap()
    };
    (state("credential"), state("model"))
}

/// Readiness follows this session's provider answers (table-manners 3.1, task 9.3): a fresh
/// server shows a set key `present`; after a scripted success the next `describe` shows it and
/// the model `valid`; after a scripted 401, the key `invalid` and the model still `valid`. The
/// script is read at each check, so rewriting it between checks scripts the session.
#[test]
fn readiness_follows_the_sessions_provider_answers() {
    let dir = out("answers");
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("script.json");
    std::fs::copy(repo_root().join("fixtures/scripts/held.json"), &script).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_squint"));
    command
        .current_dir(repo_root())
        .args([
            "serve",
            "--config",
            "fixtures/config/minimal.toml",
            "--set",
            "provider.profile=anthropic",
            "--set",
            "provider.model=claude-haiku-5-5",
            "--provider-script",
        ])
        .arg(&script)
        .arg("--out")
        .arg(dir.join("runs"))
        .env("ANTHROPIC_API_KEY", "sk-session-answers-not-a-key");
    let mut c = Client::start(&mut command).unwrap();
    c.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/images").display()
    )];
    assert_eq!(
        credential_and_model(&mut c),
        (json!("present"), json!("present"))
    );

    let held = c.call("source_fidelity", pair()).unwrap();
    assert_eq!(held.structured.unwrap()["status"], "completed");
    assert_eq!(
        credential_and_model(&mut c),
        (json!("valid"), json!("valid"))
    );

    std::fs::write(
        &script,
        r#"[{"error": {"status": 401, "message": "invalid x-api-key"}}]"#,
    )
    .unwrap();
    let rejected = c.call("source_fidelity", pair()).unwrap();
    assert_eq!(rejected.structured.unwrap()["status"], "error");
    assert_eq!(
        credential_and_model(&mut c),
        (json!("invalid"), json!("valid"))
    );
    let d = c.call("describe", json!({})).unwrap().structured.unwrap();
    assert_eq!(d["actions"][0]["readiness"]["state"], "needs_setup");
    assert!(!d.to_string().contains("sk-session-answers-not-a-key"));

    // A new server is a new session: it starts from `present` again.
    drop(c);
    let mut fresh = Client::start(&mut command).unwrap();
    assert_eq!(
        credential_and_model(&mut fresh),
        (json!("present"), json!("present"))
    );
}

/// Every file and directory under `dir`, recursively, so "unchanged" means unchanged.
fn tree(dir: &Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        all.push(entry.path());
        all.extend(tree(&entry.path()));
    }
    all.sort();
    all
}

/// Describing and estimating call no provider (table-manners 3.1, 7 item 7; task 9.3).
/// Twice over: under a scripted provider whose call counter (a file every request appends to)
/// must stay absent, and under the real Anthropic profile with a key set and its endpoint moved
/// to a local listener that must see no connection. Each part ends with a check, the control
/// that proves the counter and the listener do see a call. `describe` and `estimate` (command
/// and tool) leave the output directory unchanged: an estimate is never a run (task 9.4).
#[test]
fn describing_and_estimating_call_no_provider() {
    let dir = out("no-call");
    std::fs::create_dir_all(&dir).unwrap();
    let runs = dir.join("runs");
    let calls = dir.join("calls.log");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!("https://{}", listener.local_addr().unwrap());
    let candidate = repo_root().join("fixtures/images/candidate-64x48.png");
    let source = repo_root().join("fixtures/images/source-64x48.png");
    let squint = |config: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_squint"));
        c.current_dir(repo_root())
            .args(config)
            .env("ANTHROPIC_API_KEY", "sk-no-call-not-a-key")
            .env("SQUINT_ANTHROPIC_ACCOUNT", "org-no-call")
            .env("SQUINT_TESTKIT_PROVIDER_CALLS", &calls)
            .env("SQUINT_TESTKIT_BASE_URL", &endpoint);
        c
    };
    let nothing_connected = || {
        assert!(
            matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
            "something connected to the provider endpoint"
        );
    };
    let scripted: &[&str] = &[
        "--config",
        "fixtures/config/minimal.toml",
        "--set",
        "provider.profile=anthropic",
        "--set",
        "provider.model=claude-haiku-5-5",
        "--provider-script",
        "fixtures/scripts/held.json",
    ];
    let real: &[&str] = &[
        "--config",
        "examples/source-fidelity.toml",
        "--set",
        "budget.retries=0",
        "--set",
        "budget.request_timeout_s=30",
    ];
    for config in [scripted, real] {
        // The command line: describe, then estimate.
        let described = squint(&["describe", "--json"])
            // `describe` takes the file; the overrides are `serve`'s and `estimate`'s.
            .args(&config[..2])
            .output()
            .unwrap();
        assert_eq!(described.status.code(), Some(0));
        assert!(!String::from_utf8_lossy(&described.stdout).contains("sk-no-call-not-a-key"));
        let estimated = squint(&["estimate", "--json"])
            .args(config)
            .args(["--use-case", "source-fidelity", "--candidate"])
            .arg(&candidate)
            .arg("--source")
            .arg(&source)
            .output()
            .unwrap();
        assert_eq!(
            estimated.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&estimated.stderr)
        );
        // The server: describe, then estimate.
        let mut server = squint(&["serve"]);
        server.args(config).arg("--out").arg(&runs);
        let mut c = Client::start(&mut server).unwrap();
        c.roots = vec![format!(
            "file://{}",
            repo_root().join("fixtures/images").display()
        )];
        let before = tree(&runs);
        let d = c.call("describe", json!({})).unwrap();
        assert!(!d.is_error);
        assert_eq!(tree(&runs), before, "describe wrote nothing");
        let e = c
            .call(
                "estimate",
                json!({ "action": "source_fidelity", "arguments": pair() }),
            )
            .unwrap();
        assert!(!e.is_error, "{e:?}");
        assert_eq!(e.structured.unwrap()["allowed"], true);
        assert_eq!(tree(&runs), before, "estimate wrote nothing");
        assert!(!calls.exists(), "no provider call");
        nothing_connected();

        // The control: a check does reach the counter (scripted) or the endpoint (real).
        if config == scripted {
            let checked = c.call("source_fidelity", pair()).unwrap();
            assert_eq!(checked.structured.unwrap()["status"], "completed");
            assert!(std::fs::read_to_string(&calls).unwrap().lines().count() >= 1);
            std::fs::remove_file(&calls).unwrap();
        } else {
            // Accepted on a thread, so the client keeps answering the server (`roots/list`)
            // while the check connects; the dropped stream ends its request at once.
            let control = listener.try_clone().unwrap();
            control.set_nonblocking(false).unwrap();
            let acceptor = std::thread::spawn(move || drop(control.accept().unwrap()));
            let failed = c.call("source_fidelity", pair()).unwrap();
            acceptor.join().unwrap();
            assert_eq!(failed.structured.unwrap()["status"], "error");
        }
    }
}

const ESTIMATE: &str = "table-manners/estimate";

/// The binding of this server's estimate of a `source_fidelity` call with these arguments.
fn binding(c: &mut Client, arguments: serde_json::Value) -> String {
    let e = c
        .call(
            "estimate",
            json!({ "action": "source_fidelity", "arguments": arguments }),
        )
        .unwrap()
        .structured
        .unwrap();
    valid("urn:table-manners:core:estimate", &e);
    e["binding"].as_str().unwrap().to_string()
}

/// A check that names `binding`, ended `invalid` with `stale_estimate`: zero spend, never
/// retryable, and a run directory with its `run.json`. Returns the reason.
fn stale(c: &mut Client, arguments: serde_json::Value, binding: &str) -> String {
    let r = c
        .call_with_meta("source_fidelity", arguments, json!({ ESTIMATE: binding }))
        .unwrap();
    assert!(r.is_error, "{r:?}");
    let r = r.structured.unwrap();
    valid("urn:table-manners:core:result", &r);
    assert_eq!(r["status"], "invalid", "{r}");
    assert_eq!(r["error"]["code"], "stale_estimate", "{r}");
    assert_eq!(r["error"]["retryable"], false);
    assert_eq!(r["spend"]["usd"], 0.0);
    assert!(
        Path::new(r["run"]["dir"].as_str().unwrap())
            .join("run.json")
            .is_file()
    );
    r["error"]["reason"].as_str().unwrap().to_string()
}

/// A paid call that names an estimate is checked against it at the limits check (table-manners
/// 4.5, task 9.5): a matching binding runs; a changed image, a changed argument, another
/// session's binding (a second server) and a command-line estimate's are stale.
#[test]
fn a_call_runs_on_its_estimate_and_is_stale_on_any_other() {
    let mut c = client(&out("bound"), "held.json");
    let bound = binding(&mut c, pair());
    let ran = c
        .call_with_meta("source_fidelity", pair(), json!({ ESTIMATE: &bound }))
        .unwrap()
        .structured
        .unwrap();
    assert_eq!(ran["status"], "completed", "{ran}");

    let mut other_image = pair();
    other_image["candidate"] =
        json!({ "path": repo_root().join("fixtures/images/exif-rotated-64x48.png") });
    stale(&mut c, other_image, &bound);
    let mut briefed = pair();
    briefed["brief"] = json!("keep the sky");
    stale(&mut c, briefed, &bound);

    let mut second = client(&out("bound-second"), "held.json");
    stale(&mut second, pair(), &bound);

    let cli = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "estimate",
            "--json",
            "--config",
            "fixtures/config/minimal.toml",
            "--provider-script",
            "fixtures/scripts/held.json",
            "--use-case",
            "source-fidelity",
            "--candidate",
            "fixtures/images/candidate-64x48.png",
            "--source",
            "fixtures/images/source-64x48.png",
        ])
        .output()
        .unwrap();
    assert_eq!(cli.status.code(), Some(0));
    let cli: serde_json::Value = serde_json::from_slice(&cli.stdout).unwrap();
    stale(&mut c, pair(), cli["binding"].as_str().unwrap());
}

/// A binding past its `expires` is stale (the testkit clock seam makes estimates expire at once).
#[test]
fn an_expired_estimate_is_stale() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_squint"));
    command
        .current_dir(repo_root())
        .args([
            "serve",
            "--config",
            "fixtures/config/minimal.toml",
            "--provider-script",
            "fixtures/scripts/held.json",
            "--out",
        ])
        .arg(out("expired"))
        .env("SQUINT_TESTKIT_ESTIMATE_TTL_S", "0");
    let mut c = Client::start(&mut command).unwrap();
    c.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/images").display()
    )];
    let bound = binding(&mut c, pair());
    let reason = stale(&mut c, pair(), &bound);
    assert!(reason.contains("expired"), "{reason}");
}
