//! Task 7.7a: `squint serve` under the contract's server conventions (table-manners 5), driven by
//! the independent test client: the handshake at `2025-11-25` only, the tools listed through the
//! cursor, `describe` equal to `squint describe --json`, `estimate` as a run, unknown tools as
//! JSON-RPC errors, and nothing but the protocol on stdout.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use squint_testkit::mcp_client::{Client, Failure, PROTOCOL};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn out(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-serve-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn server(out: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_squint"));
    c.current_dir(repo_root())
        .args([
            "serve",
            "--config",
            "examples/source-fidelity.toml",
            "--out",
        ])
        .arg(out)
        .env_remove("ANTHROPIC_API_KEY")
        .env("SQUINT_ANTHROPIC_ACCOUNT", "org-serve");
    c
}

fn image(name: &str) -> serde_json::Value {
    json!({ "path": repo_root().join("fixtures/images").join(name) })
}

#[test]
fn the_handshake_is_at_2025_11_25_with_tools() {
    let client = Client::start(&mut server(&out("handshake"))).unwrap();
    assert_eq!(client.server["protocolVersion"], PROTOCOL);
    assert_eq!(client.server["serverInfo"]["name"], "squint");
    assert!(!client.server["capabilities"]["tools"].is_null());
    assert!(client.close().unwrap().success());
}

/// A client offering another revision is answered with the only one squint speaks, so it can
/// see it cannot meet the server and disconnect.
#[test]
fn another_revision_is_answered_with_the_only_one_squint_speaks() {
    let client = Client::start_at(&mut server(&out("revision")), "2025-06-18").unwrap();
    assert_eq!(client.server["protocolVersion"], PROTOCOL);
}

#[test]
fn describe_is_a_tool_and_equals_the_command() {
    let mut client = Client::start(&mut server(&out("describe"))).unwrap();
    let names: Vec<String> = client
        .list_tools()
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(names.contains(&"describe".to_string()), "{names:?}");
    assert!(names.contains(&"estimate".to_string()), "{names:?}");
    let described = client.call("describe", json!({})).unwrap();
    assert!(!described.is_error);
    let command = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "describe",
            "--json",
            "--config",
            "examples/source-fidelity.toml",
        ])
        .env_remove("ANTHROPIC_API_KEY")
        .env("SQUINT_ANTHROPIC_ACCOUNT", "org-serve")
        .output()
        .unwrap();
    let expected: serde_json::Value = serde_json::from_slice(&command.stdout).unwrap();
    assert_eq!(described.structured.unwrap(), expected);
}

#[test]
fn estimate_is_a_run_with_its_own_run_json() {
    let dir = out("estimate");
    let mut client = Client::start(&mut server(&dir)).unwrap();
    // An estimate reads its images, so they must be inside the client's roots like a check's.
    client.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/images").display()
    )];
    let r = client
        .call(
            "estimate",
            json!({
                "use_case": "source-fidelity",
                "candidate": image("candidate-64x48.png"),
                "source": image("source-64x48.png"),
            }),
        )
        .unwrap();
    assert!(!r.is_error, "{r:?}");
    let result = r.structured.unwrap();
    assert_eq!(result["status"], "completed");
    assert_eq!(result["action"], "estimate");
    assert!(result["body"]["envelope_usd"].as_f64().unwrap() > 0.0);
    let run_dir = PathBuf::from(result["run"]["dir"].as_str().unwrap());
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(on_disk, result);

    // Every run resource the server advertises is readable for this run, by this server and by a
    // new one on the same output (wafflehouse estimate-journal handoff): an estimate has no
    // decision journal, so its journal is empty, not missing. An unknown run stays unavailable.
    let id = result["run"]["id"].as_str().unwrap().to_string();
    let journal = format!("squint://run/{id}/journal");
    assert_eq!(client.read_resource(&journal).unwrap(), "");
    assert!(
        client
            .read_resource("squint://run/no-such-run/journal")
            .is_err()
    );
    drop(client);
    let mut again = Client::start(&mut server(&dir)).unwrap();
    assert_eq!(again.read_resource(&journal).unwrap(), "");
}

/// A relative path is a run that is `invalid` with code `input`; arguments the input schema
/// rejects and unknown tools are JSON-RPC errors, since no run exists.
#[test]
fn bad_calls_are_invalid_results_or_json_rpc_errors() {
    let mut client = Client::start(&mut server(&out("bad"))).unwrap();
    let r = client
        .call(
            "estimate",
            json!({ "use_case": "source-fidelity", "candidate": { "path": "relative.png" } }),
        )
        .unwrap();
    assert!(r.is_error);
    let result = r.structured.unwrap();
    assert_eq!(result["status"], "invalid");
    assert_eq!(result["error"]["code"], "input");
    assert!(matches!(
        client.call(
            "estimate",
            json!({ "use_case": "source-fidelity", "colour": "red" })
        ),
        Err(Failure::Rpc { .. })
    ));
    assert!(matches!(
        client.call("no_such_tool", json!({})),
        Err(Failure::Rpc { .. })
    ));
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

/// `squint serve` with no configuration anywhere on the lookup: run in an empty working
/// directory with the user's config directory empty too, and `extra` arguments.
fn unconfigured(tag: &str, extra: &[&std::ffi::OsStr]) -> (PathBuf, Command) {
    let dir = out(&format!("unconfigured-{tag}"));
    for sub in ["work", "xdg", "home"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    let mut c = Command::new(env!("CARGO_BIN_EXE_squint"));
    c.current_dir(dir.join("work"))
        .arg("serve")
        .args(extra)
        .env("XDG_CONFIG_HOME", dir.join("xdg"))
        .env("HOME", dir.join("home"));
    (dir, c)
}

/// An unconfigured server starts (table-manners 3.1, 5; task 9.2): it completes `initialize`,
/// lists its tools, answers `describe` with every action needing the configuration, and refuses
/// a check and an estimate `not_ready` before any run, so nothing is written (not even its
/// output directory) and the request's `client_run_id` is released.
#[test]
fn an_unconfigured_server_starts_and_refuses_runs_not_ready() {
    let runs = out("unconfigured-runs");
    let (dir, mut command) = unconfigured("refuses", &["--out".as_ref(), runs.as_os_str()]);
    let mut client = Client::start(&mut command).unwrap();
    client.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/images").display()
    )];
    let names: Vec<String> = client
        .list_tools()
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert!(names.contains(&"describe".to_string()), "{names:?}");
    assert!(names.contains(&"source_fidelity".to_string()), "{names:?}");
    let described = client
        .call("describe", json!({}))
        .unwrap()
        .structured
        .unwrap();
    valid("urn:table-manners:core:description", &described);
    let path = dir.join("xdg/squint/squint.toml");
    for action in described["actions"].as_array().unwrap() {
        assert_eq!(action["readiness"]["state"], "needs_setup");
        assert_eq!(
            action["readiness"]["requirements"][0]["name"],
            path.to_str().unwrap()
        );
    }
    let pair = json!({
        "candidate": image("candidate-64x48.png"),
        "source": image("source-64x48.png"),
    });
    let mut estimate = pair.clone();
    estimate["use_case"] = json!("source-fidelity");
    for (tool, arguments) in [("source_fidelity", &pair), ("estimate", &estimate)] {
        // The same id twice: a refusal releases it, so the second is refused for readiness again
        // rather than as a reuse.
        for _ in 0..2 {
            let refused = client.call_with_meta(
                tool,
                arguments.clone(),
                json!({ "table-manners/client_run_id": "unconfigured-1" }),
            );
            let Err(Failure::Rpc { code, data, .. }) = refused else {
                panic!("{tool}: {refused:?}");
            };
            assert_eq!(code, -32602);
            let data = data.unwrap();
            valid("urn:table-manners:core:refusal", &data);
            assert_eq!(data["reason"], "not_ready", "{data}");
            assert_eq!(data["run"], serde_json::Value::Null);
            assert_eq!(data["requirements"][0]["kind"], "config");
            assert_eq!(data["requirements"][0]["state"], "missing");
        }
    }
    let listed: serde_json::Value =
        serde_json::from_str(&client.read_resource("squint://runs").unwrap()).unwrap();
    assert_eq!(listed["runs"], json!([]));
    assert!(client.close().unwrap().success());
    assert!(!runs.exists(), "an unconfigured server writes nothing");
    assert_eq!(std::fs::read_dir(dir.join("work")).unwrap().count(), 0);
}

/// Without `--out` either, the server has no output directory and still answers.
#[test]
fn an_unconfigured_server_needs_no_output_directory() {
    let (dir, mut command) = unconfigured("no-out", &[]);
    let mut client = Client::start(&mut command).unwrap();
    let listed: serde_json::Value =
        serde_json::from_str(&client.read_resource("squint://runs").unwrap()).unwrap();
    assert_eq!(listed["runs"], json!([]));
    let session: serde_json::Value =
        serde_json::from_str(&client.read_resource("squint://session").unwrap()).unwrap();
    valid("urn:table-manners:core:session", &session);
    assert!(client.close().unwrap().success());
    assert_eq!(std::fs::read_dir(dir.join("work")).unwrap().count(), 0);
}

/// A configuration that does not parse starts the server the same way, `invalid`.
#[test]
fn a_server_with_an_invalid_config_starts_and_says_so() {
    let broken = out("unconfigured-broken.toml");
    std::fs::write(&broken, "[provider\n").unwrap();
    let (_, mut command) = unconfigured("broken", &["--config".as_ref(), broken.as_os_str()]);
    let mut client = Client::start(&mut command).unwrap();
    let described = client
        .call("describe", json!({}))
        .unwrap()
        .structured
        .unwrap();
    let requirement = &described["actions"][0]["readiness"]["requirements"][0];
    assert_eq!(requirement["state"], "invalid", "{requirement}");
    assert_eq!(requirement["name"], broken.to_str().unwrap());
}
