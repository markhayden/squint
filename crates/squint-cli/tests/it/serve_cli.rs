//! Task 7.7a: `squint serve` under the contract's server conventions (table-manners 5), driven by
//! the independent test client: the handshake at `2025-11-25` only, the tools listed through the
//! cursor, `describe` equal to `squint describe --json`, unknown tools as JSON-RPC errors, and
//! nothing but the protocol on stdout. Task 9.4: `estimate`, the reserved tool, never a run.
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

/// An estimate is a control, never a run (table-manners 4.5, task 9.4): it is a tool and not an
/// action, it validates against `estimate.json`, and the output directory, `squint://runs` and
/// the session's spent and reserved are as they were. With the key unset it is priced and still
/// refused for readiness, bound to this session.
#[test]
fn estimate_is_never_a_run() {
    let dir = out("estimate");
    std::fs::create_dir_all(&dir).unwrap();
    let mut client = Client::start(&mut server(&dir)).unwrap();
    // An estimate reads its images, so they must be inside the client's roots like a check's.
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
    assert!(names.contains(&"estimate".to_string()), "{names:?}");
    let described = client
        .call("describe", json!({}))
        .unwrap()
        .structured
        .unwrap();
    assert!(
        described["actions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["name"] != "estimate")
    );
    let session = |c: &mut Client| -> serde_json::Value {
        serde_json::from_str(&c.read_resource("squint://session").unwrap()).unwrap()
    };
    let runs = |c: &mut Client| -> serde_json::Value {
        serde_json::from_str(&c.read_resource("squint://runs").unwrap()).unwrap()
    };
    let before = (tree(&dir), runs(&mut client), session(&mut client));
    let r = client
        .call(
            "estimate",
            json!({
                "action": "source_fidelity",
                "arguments": {
                    "candidate": image("candidate-64x48.png"),
                    "source": image("source-64x48.png"),
                },
            }),
        )
        .unwrap();
    assert!(!r.is_error, "{r:?}");
    let estimate = r.structured.unwrap();
    valid("urn:table-manners:core:estimate", &estimate);
    assert_eq!(estimate["action"], "source_fidelity");
    assert_eq!(estimate["session_id"], before.2["id"]);
    assert_eq!(estimate["session"]["id"], before.2["id"]);
    assert_eq!(estimate["effective"]["model"], "claude-haiku-5-5");
    assert!(
        estimate["worst_case_usd"].as_f64().unwrap() > 0.0,
        "{estimate}"
    );
    assert_eq!(estimate["cap_usd"], 2.0);
    // ANTHROPIC_API_KEY is unset: the call is priced and refused for readiness.
    assert_eq!(estimate["allowed"], false);
    assert_eq!(estimate["refused"], "readiness");
    assert_eq!(estimate["readiness"]["state"], "needs_setup");
    let after = (tree(&dir), runs(&mut client), session(&mut client));
    assert_eq!(after.0, before.0, "an estimate writes nothing");
    assert_eq!(after.1, before.1, "an estimate is not in the runs");
    for key in ["spent_usd", "reserved_usd", "unresolved_usd"] {
        assert_eq!(after.2[key], before.2[key], "{key}");
    }
}

/// An estimate's wrong requests are JSON-RPC errors, since no run exists (table-manners 4.5):
/// a relative path is `-32602` with the `invalid_arguments` refusal (`input`), arguments the
/// input schema rejects are plain `-32602`; so are unknown tools.
#[test]
fn bad_calls_are_json_rpc_errors() {
    let mut client = Client::start(&mut server(&out("bad"))).unwrap();
    let relative = client.call(
        "estimate",
        json!({ "action": "source_fidelity",
                "arguments": { "candidate": { "path": "relative.png" } } }),
    );
    let Err(Failure::Rpc { code, data, .. }) = relative else {
        panic!("{relative:?}");
    };
    assert_eq!(code, -32602);
    let data = data.unwrap();
    valid("urn:table-manners:core:refusal", &data);
    assert_eq!(data["reason"], "invalid_arguments");
    assert_eq!(data["error"]["code"], "input");
    for arguments in [
        json!({ "action": "source_fidelity", "arguments": { "colour": "red" } }),
        json!({ "action": "source_fidelity" }),
        json!({ "arguments": {} }),
    ] {
        let r = client.call("estimate", arguments.clone());
        let Err(Failure::Rpc { code, data, .. }) = r else {
            panic!("{arguments}: {r:?}");
        };
        assert_eq!(code, -32602, "{arguments}");
        assert!(data.is_none(), "{arguments}: {data:?}");
    }
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
/// a check `not_ready` before any run, so nothing is written (not even its output directory) and
/// the request's `client_run_id` is released. Its estimate cannot be priced: refused for
/// readiness with the worst case, the model and the configuration's identity null (4.5).
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
    // The same id twice: a refusal releases it, so the second is refused for readiness again
    // rather than as a reuse.
    for _ in 0..2 {
        let refused = client.call_with_meta(
            "source_fidelity",
            pair.clone(),
            json!({ "table-manners/client_run_id": "unconfigured-1" }),
        );
        let Err(Failure::Rpc { code, data, .. }) = refused else {
            panic!("{refused:?}");
        };
        assert_eq!(code, -32602);
        let data = data.unwrap();
        valid("urn:table-manners:core:refusal", &data);
        assert_eq!(data["reason"], "not_ready", "{data}");
        assert_eq!(data["run"], serde_json::Value::Null);
        assert_eq!(data["requirements"][0]["kind"], "config");
        assert_eq!(data["requirements"][0]["state"], "missing");
    }
    let estimate = client
        .call(
            "estimate",
            json!({ "action": "source_fidelity", "arguments": pair }),
        )
        .unwrap()
        .structured
        .unwrap();
    valid("urn:table-manners:core:estimate", &estimate);
    assert_eq!(estimate["allowed"], false);
    assert_eq!(estimate["refused"], "readiness");
    assert_eq!(estimate["worst_case_usd"], serde_json::Value::Null);
    assert_eq!(estimate["effective"]["model"], serde_json::Value::Null);
    assert_eq!(
        estimate["effective"]["config_identity"],
        serde_json::Value::Null
    );
    assert_eq!(estimate["readiness"]["requirements"][0]["state"], "missing");
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

/// A server on the scripted provider, ready and priced from its own prices, its roots the fixture
/// images.
#[cfg(feature = "testkit")]
fn scripted(out: &Path, overrides: &[&str]) -> Client {
    let mut c = Command::new(env!("CARGO_BIN_EXE_squint"));
    c.current_dir(repo_root())
        .args([
            "serve",
            "--config",
            "fixtures/config/minimal.toml",
            "--provider-script",
            "fixtures/scripts/held.json",
            "--out",
        ])
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

#[cfg(feature = "testkit")]
fn inline(name: &str) -> serde_json::Value {
    let bytes = std::fs::read(repo_root().join("fixtures/images").join(name)).unwrap();
    json!({ "inline": { "media_type": "image/png", "data": squint_core::image::base64_encode(&bytes) } })
}

/// The estimate of a `source_fidelity` call with these arguments, valid against `estimate.json`.
#[cfg(feature = "testkit")]
fn estimate_of(c: &mut Client, arguments: serde_json::Value) -> serde_json::Value {
    let r = c
        .call(
            "estimate",
            json!({ "action": "source_fidelity", "arguments": arguments }),
        )
        .unwrap();
    assert!(!r.is_error, "{r:?}");
    let estimate = r.structured.unwrap();
    valid("urn:table-manners:core:estimate", &estimate);
    estimate
}

/// The binding covers the call and nothing else (table-manners 4.5, task 9.4): a path and inline
/// bytes of the same file bind equal, and so do calls that differ only in how results are
/// delivered; a changed image, argument or `max_spend_usd` binds differently.
#[cfg(feature = "testkit")]
#[test]
fn a_binding_follows_the_call_and_nothing_else() {
    let mut c = scripted(&out("binding"), &[]);
    let call = |candidate: serde_json::Value| json!({ "candidate": candidate, "source": image("source-64x48.png") });
    let by_path = estimate_of(&mut c, call(image("candidate-64x48.png")));
    assert_eq!(by_path["allowed"], true, "{by_path}");
    assert_eq!(by_path["refused"], serde_json::Value::Null);
    assert_eq!(by_path["effective"]["x-squint"]["decision_runs"], 1);
    let binding = by_path["binding"].clone();
    let by_inline = estimate_of(&mut c, call(inline("candidate-64x48.png")));
    assert_eq!(by_inline["binding"], binding, "path and inline bind equal");
    let mut delivered = call(image("candidate-64x48.png"));
    delivered["return_images"] = json!("inline");
    delivered["image_blocks"] = json!(true);
    assert_eq!(estimate_of(&mut c, delivered)["binding"], binding);
    let mut briefed = call(image("candidate-64x48.png"));
    briefed["brief"] = json!("keep the sky");
    let mut capped = call(image("candidate-64x48.png"));
    capped["max_spend_usd"] = json!(1.5);
    for changed in [
        call(image("exif-rotated-64x48.png")),
        briefed,
        capped.clone(),
    ] {
        assert_ne!(
            estimate_of(&mut c, changed.clone())["binding"],
            binding,
            "{changed}"
        );
    }
    assert_eq!(estimate_of(&mut c, capped)["cap_usd"], 1.5);
}

/// The limits an estimate reports (table-manners 4.5): a cap below the worst case is `cap`, a
/// session that cannot hold it is `budget`, readiness first.
#[cfg(feature = "testkit")]
#[test]
fn an_estimate_reports_the_cap_and_the_budget() {
    let pair = || json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") });
    let mut c = scripted(&out("limits"), &[]);
    let mut low = pair();
    low["max_spend_usd"] = json!(0.000001);
    let capped = estimate_of(&mut c, low);
    assert_eq!(
        (capped["allowed"].clone(), capped["refused"].clone()),
        (json!(false), json!("cap"))
    );
    let mut c = scripted(&out("budget"), &["serve.session_budget_usd=0.000001"]);
    let spent = estimate_of(&mut c, pair());
    assert_eq!(
        (spent["allowed"].clone(), spent["refused"].clone()),
        (json!(false), json!("budget"))
    );
}

/// An estimate's wrong requests other than the schema's are `-32602` with the
/// `invalid_arguments` refusal (table-manners 4.5): an `expect` that does not match (`input`), an
/// unknown action and `max_spend_usd` above the configured cap (`usage`).
#[cfg(feature = "testkit")]
#[test]
fn an_estimates_wrong_request_is_the_invalid_arguments_refusal() {
    let mut c = scripted(&out("wrong"), &[]);
    let expect = json!({ "file_sha256": "0".repeat(64), "width": 64, "height": 48,
                         "projection": "flat", "topology": "bounded" });
    let candidate = repo_root().join("fixtures/images/candidate-64x48.png");
    let pair = || json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") });
    let mut over = pair();
    over["max_spend_usd"] = json!(5.0);
    for (request, code) in [
        (
            json!({ "action": "source_fidelity",
                    "arguments": { "candidate": { "path": candidate, "expect": expect },
                                   "source": image("source-64x48.png") } }),
            "input",
        ),
        (
            json!({ "action": "no_such_action", "arguments": pair() }),
            "usage",
        ),
        (
            json!({ "action": "source_fidelity", "arguments": over }),
            "usage",
        ),
    ] {
        let r = c.call("estimate", request.clone());
        let Err(Failure::Rpc {
            code: rpc, data, ..
        }) = r
        else {
            panic!("{request}: {r:?}");
        };
        assert_eq!(rpc, -32602, "{request}");
        let data = data.unwrap();
        valid("urn:table-manners:core:refusal", &data);
        assert_eq!(data["reason"], "invalid_arguments", "{request}");
        assert_eq!(data["run"], serde_json::Value::Null);
        assert_eq!(data["error"]["code"], code, "{request}");
        assert_eq!(data["error"]["retryable"], false);
    }
}
