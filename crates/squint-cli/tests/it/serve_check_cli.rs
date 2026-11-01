//! Task 7.7b: a use case's action over `squint serve` is one decision (table-manners 5, 10):
//! images by path inside the client's roots or inline, the result the decision's `run.json`,
//! `invalid` for a path outside the roots, and the session budget refusing before any request;
//! since task 9.8, `references` a `many` slot: a non-empty list, taken in once per image in order.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use squint_testkit::mcp_client::{Client, Failure};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn out(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-serve-check-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn server(out: &Path, overrides: &[&str]) -> Command {
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
    c
}

/// A client whose roots hold the fixture images.
fn client(out: &Path, overrides: &[&str]) -> Client {
    let mut client = Client::start(&mut server(out, overrides)).unwrap();
    client.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/images").display()
    )];
    client
}

fn image(name: &str) -> serde_json::Value {
    json!({ "path": repo_root().join("fixtures/images").join(name) })
}

fn inline(name: &str) -> serde_json::Value {
    let bytes = std::fs::read(repo_root().join("fixtures/images").join(name)).unwrap();
    json!({ "inline": { "media_type": "image/png", "data": squint_core::image::base64_encode(&bytes) } })
}

#[test]
fn every_use_case_is_a_tool() {
    let mut c = client(&out("tools"), &[]);
    let names: Vec<String> = c
        .list_tools()
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    for want in [
        "source_fidelity",
        "reference_consistency",
        "reality_checks",
        "brief_compliance",
        "product_fidelity",
    ] {
        assert!(names.contains(&want.to_string()), "{want}: {names:?}");
    }
}

#[test]
fn a_check_by_path_and_by_inline_bytes_reaches_the_same_verdict() {
    let mut c = client(&out("check"), &[]);
    let by_path = c
        .call(
            "source_fidelity",
            json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") }),
        )
        .unwrap();
    assert!(!by_path.is_error, "{by_path:?}");
    let r = by_path.structured.unwrap();
    assert_eq!(r["status"], "completed");
    assert_eq!(r["action"], "source_fidelity");
    let dir = PathBuf::from(r["run"]["dir"].as_str().unwrap());
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(on_disk, r);
    let mut c = client(&out("check-inline"), &[]);
    let by_inline = c
        .call(
            "source_fidelity",
            json!({ "candidate": inline("candidate-64x48.png"), "source": inline("source-64x48.png") }),
        )
        .unwrap()
        .structured
        .unwrap();
    // The same bytes are the same inputs, whichever way they came.
    assert_eq!(by_inline["body"]["verdict"], r["body"]["verdict"]);
    assert_eq!(by_inline["images"]["in"], r["images"]["in"]);
    assert_eq!(by_inline["locations"], r["locations"]);
    assert_eq!(by_inline["input_identity"], r["input_identity"]);
}

#[test]
fn a_path_outside_the_clients_roots_is_invalid_input_and_spends_nothing() {
    let mut c = client(&out("roots"), &[]);
    c.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/jobs").display()
    )];
    let r = c
        .call(
            "source_fidelity",
            json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") }),
        )
        .unwrap();
    assert!(r.is_error);
    let r = r.structured.unwrap();
    assert_eq!(r["status"], "invalid");
    assert_eq!(r["error"]["code"], "input");
    assert_eq!(r["spend"]["usd"], 0.0);
}

/// `estimate` reads its images too, so it answers to the same roots as a check (review C1): a
/// path outside them is refused before it is opened, and a missing path is refused in the same
/// words, so nothing about the file (its digest, size or existence) comes back.
#[test]
fn estimate_refuses_a_path_outside_the_roots_without_reading_it() {
    // A catalog model, so an estimate that ignored the roots would go on to read the image.
    let mut c = client(&out("estimate-roots"), &["provider.model=claude-haiku-5-5"]);
    c.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/jobs").display()
    )];
    let expect = json!({ "file_sha256": "0".repeat(64), "width": 64, "height": 48,
                         "projection": "flat", "topology": "bounded" });
    let outside = repo_root().join("fixtures/images/candidate-64x48.png");
    let missing = repo_root().join("fixtures/images/no-such-image.png");
    let mut reasons = Vec::new();
    for path in [&outside, &missing] {
        let r = c
            .call(
                "estimate",
                json!({ "use_case": "source-fidelity",
                        "candidate": { "path": path, "expect": expect },
                        "source": image("source-64x48.png") }),
            )
            .unwrap();
        assert!(r.is_error, "{path:?}");
        let r = r.structured.unwrap();
        assert_eq!(r["status"], "invalid", "{r}");
        assert_eq!(r["error"]["code"], "input", "{r}");
        let reason = r["error"]["reason"].as_str().unwrap().to_string();
        assert!(!reason.contains("file_sha256"), "{reason}");
        reasons.push(reason.replace(&path.display().to_string(), "<path>"));
    }
    assert_eq!(
        reasons[0], reasons[1],
        "a missing file must read like one outside"
    );
}

/// A decision that fails after its directory exists is answered with that decision's own run
/// (review I2): one run in `squint://runs`, ended, never a second empty one beside it.
#[test]
fn a_late_failure_is_answered_with_the_decisions_own_run() {
    let dir = out("late");
    std::fs::create_dir_all(&dir).unwrap();
    let not_a_dir = dir.join("cache-is-a-file");
    std::fs::write(&not_a_dir, "").unwrap();
    let cache = format!("cache.dir={:?}", not_a_dir.display().to_string());
    let mut c = client(&dir.join("runs"), &[&cache]);
    let r = c
        .call(
            "source_fidelity",
            json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") }),
        )
        .unwrap();
    assert!(r.is_error);
    let r = r.structured.unwrap();
    assert_eq!(r["status"], "error", "{r}");
    assert_eq!(r["error"]["code"], "io", "{r}");
    let runs: serde_json::Value =
        serde_json::from_str(&c.read_resource("squint://runs").unwrap()).unwrap();
    let runs = runs["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!(runs[0]["id"], r["run"]["id"]);
    assert_eq!(runs[0]["status"], "error");
}

/// A config that names its rules serves only that use case, and its relative rules files resolve
/// against the working directory as `squint check` resolves them (review I6).
#[test]
fn a_config_with_rules_serves_its_use_case_with_its_files() {
    let dir = out("rules");
    std::fs::create_dir_all(&dir).unwrap();
    let minimal =
        std::fs::read_to_string(repo_root().join("fixtures/config/minimal.toml")).unwrap();
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        format!("{minimal}\n[rules]\nbase = \"source-fidelity\"\nextra = [\"fixtures/rules/mine.toml\"]\n"),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_squint"));
    command
        .current_dir(repo_root())
        .args(["serve", "--config"])
        .arg(&config)
        .args(["--provider-script", "fixtures/scripts/held.json", "--out"])
        .arg(dir.join("runs"));
    let mut c = Client::start(&mut command).unwrap();
    c.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/images").display()
    )];
    let names: Vec<String> = c
        .list_tools()
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["describe", "source_fidelity", "estimate"]);
    let r = c
        .call(
            "source_fidelity",
            json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") }),
        )
        .unwrap();
    let r = r.structured.unwrap();
    assert_eq!(r["status"], "completed", "{r}");
}

/// Roots are `file:` URLs (RFC 8089): a percent-encoded space and a `localhost` host name the
/// same directory as the plain path (review I5).
#[test]
fn roots_are_read_as_file_urls() {
    let dir = out("root-urls");
    let images = dir.join("my images");
    std::fs::create_dir_all(&images).unwrap();
    for name in ["candidate-64x48.png", "source-64x48.png"] {
        std::fs::copy(
            repo_root().join("fixtures/images").join(name),
            images.join(name),
        )
        .unwrap();
    }
    let encoded = images.display().to_string().replace(' ', "%20");
    for root in [
        format!("file://{encoded}"),
        format!("file://localhost{encoded}"),
    ] {
        let mut c = client(&dir.join("runs"), &[]);
        c.roots = vec![root.clone()];
        let r = c
            .call(
                "source_fidelity",
                json!({ "candidate": { "path": images.join("candidate-64x48.png") },
                        "source": { "path": images.join("source-64x48.png") } }),
            )
            .unwrap();
        let r = r.structured.unwrap();
        assert_eq!(r["status"], "completed", "{root}: {r}");
    }
}

/// With no roots from the client and none in `[serve] roots`, a path names nothing squint may read
/// (table-manners 10.1): closed by default. Inline images still work.
#[test]
fn with_no_roots_anywhere_a_path_is_refused() {
    let mut c = Client::start_without_roots(&mut server(&out("no-roots"), &[])).unwrap();
    let r = c
        .call(
            "source_fidelity",
            json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") }),
        )
        .unwrap();
    let r = r.structured.unwrap();
    assert_eq!(r["status"], "invalid", "{r}");
    assert_eq!(r["error"]["code"], "input", "{r}");
    let r = c
        .call(
            "source_fidelity",
            json!({ "candidate": inline("candidate-64x48.png"), "source": inline("source-64x48.png") }),
        )
        .unwrap();
    assert_eq!(r.structured.unwrap()["status"], "completed");
}

#[test]
fn an_exhausted_session_budget_is_a_budget_error_before_any_request() {
    let mut c = client(&out("budget"), &["serve.session_budget_usd=0.000001"]);
    let r = c
        .call(
            "source_fidelity",
            json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") }),
        )
        .unwrap();
    assert!(r.is_error);
    let r = r.structured.unwrap();
    assert_eq!(r["status"], "error");
    assert_eq!(r["error"]["code"], "budget");
    assert_eq!(r["spend"]["usd"], 0.0);
}

#[test]
fn no_image_blocks_unless_asked_and_none_to_send() {
    let mut c = client(&out("blocks"), &[]);
    let r = c
        .call(
            "source_fidelity",
            json!({
                "candidate": image("candidate-64x48.png"),
                "source": image("source-64x48.png"),
                "image_blocks": true
            }),
        )
        .unwrap();
    assert!(r.images.is_empty(), "squint returns no images");
}

/// Every tool's `inputSchema` from `tools/list` compiles as a document of its own, the way a
/// strict client receives it (squint issue #2): no reference points outside it.
#[test]
fn every_tool_input_schema_compiles_on_its_own() {
    let mut c = client(&out("input-schemas"), &[]);
    let tools = c.list_tools().unwrap();
    assert!(tools.len() >= 7, "{tools:?}");
    for tool in tools {
        jsonschema::draft202012::new(&tool.input_schema)
            .unwrap_or_else(|e| panic!("{}: {e}", tool.name));
    }
}

/// A server running the scripted provider declares what it can do: no check spends (squint
/// issue #2), whatever profile the config names. Its readiness is still the configured
/// profile's (task 9.3): the script stands in for that endpoint, so the configuration, the
/// profile's credential by name and the model are listed.
#[test]
fn a_scripted_server_declares_free_checks() {
    let mut command = server(
        &out("free"),
        &[
            "provider.profile=anthropic",
            "provider.model=claude-haiku-5-5",
        ],
    );
    command.env("ANTHROPIC_API_KEY", "sk-scripted-not-a-key");
    let mut c = Client::start(&mut command).unwrap();
    let d = c.call("describe", json!({})).unwrap().structured.unwrap();
    assert!(!d.to_string().contains("sk-scripted-not-a-key"));
    for action in d["actions"].as_array().unwrap() {
        assert_eq!(action["cost"]["may_spend"], false, "{}", action["name"]);
        assert_eq!(
            action["readiness"],
            json!({ "state": "ready", "requirements": [
                { "kind": "config", "name": "fixtures/config/minimal.toml", "state": "present" },
                {
                    "kind": "credential",
                    "name": "ANTHROPIC_API_KEY",
                    "state": "present",
                    "setup": "Set ANTHROPIC_API_KEY in the environment squint is started with."
                },
                { "kind": "model", "name": "claude-haiku-5-5", "state": "present" }
            ] }),
            "{}",
            action["name"]
        );
    }
}

/// `references` is a `many` slot (table-manners 10.1, 11.2): every tool that takes it declares a
/// non-empty array of images in, and the slots that take one image declare an object.
#[test]
fn the_references_input_is_a_non_empty_array_of_images() {
    let mut c = client(&out("many-schema"), &[]);
    let tools = c.list_tools().unwrap();
    let checks: Vec<_> = tools
        .iter()
        .filter(|t| t.input_schema["properties"].get("candidate").is_some())
        .collect();
    assert!(checks.len() >= 6, "{tools:?}");
    for tool in checks {
        let properties = &tool.input_schema["properties"];
        assert_eq!(properties["references"]["type"], "array", "{}", tool.name);
        assert_eq!(properties["references"]["minItems"], 1, "{}", tool.name);
        assert_eq!(
            properties["references"]["items"], properties["candidate"],
            "{}",
            tool.name
        );
        assert_eq!(properties["candidate"]["type"], "object", "{}", tool.name);
    }
}

/// A check with three references takes each in through the `references` slot, once per image, in
/// the order given (table-manners 11.1). `images.in` is filled when the images are taken in, so
/// the scripted verdict (written for source fidelity, which ends this run in a provider error) does
/// not matter here.
#[test]
fn three_references_are_taken_in_once_each_in_order() {
    let mut c = client(&out("many-in"), &[]);
    let references = [
        "source-64x48.png",
        "two-to-one-128x64.png",
        "candidate-64x48.png",
    ];
    let r = c
        .call(
            "reference_consistency",
            json!({
                "candidate": image("candidate-64x48.png"),
                "references": references.map(image),
            }),
        )
        .unwrap()
        .structured
        .unwrap();
    assert_ne!(r["status"], "invalid", "{r}");
    let taken: Vec<(&str, &str, &str)> = r["images"]["in"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            (
                i["slot"].as_str().unwrap(),
                i["role"].as_str().unwrap(),
                i["image"]["file_sha256"].as_str().unwrap(),
            )
        })
        .collect();
    let digest = |name: &str| {
        squint_core::image::sha256_hex(
            &std::fs::read(repo_root().join("fixtures/images").join(name)).unwrap(),
        )
    };
    let want_refs: Vec<String> = references.iter().map(|n| digest(n)).collect();
    assert_eq!(taken.len(), 4, "{taken:?}");
    assert_eq!(taken[0].0, "candidate");
    for (n, (slot, role, sha)) in taken[1..].iter().enumerate() {
        assert_eq!((*slot, *role), ("references", "reference"), "{n}");
        assert!(
            sha.ends_with(&want_refs[n]),
            "{n}: {sha} is not {}",
            references[n]
        );
    }
}

/// An empty `references` list is a JSON-RPC error before any run (table-manners 10.1: a `many`
/// slot takes a non-empty array), and it releases its `client_run_id` for the corrected request.
#[test]
fn an_empty_references_list_is_refused_before_any_run_and_releases_its_id() {
    const CLIENT: &str = "table-manners/client_run_id";
    let out = out("many-empty");
    let mut c = client(&out, &[]);
    let call = |references: serde_json::Value| {
        json!({
            "candidate": image("candidate-64x48.png"),
            "references": references,
        })
    };
    match c.call_with_meta(
        "reference_consistency",
        call(json!([])),
        json!({ CLIENT: "empty-1" }),
    ) {
        Err(Failure::Rpc { code, message, .. }) => {
            assert_eq!(code, -32602);
            assert!(message.contains("references"), "{message}");
        }
        other => panic!("expected a JSON-RPC error, got {other:?}"),
    }
    let runs: serde_json::Value =
        serde_json::from_str(&c.read_resource("squint://runs").unwrap()).unwrap();
    assert_eq!(runs["runs"], json!([]), "a refusal makes no run");
    let accepted = c
        .call_with_meta(
            "reference_consistency",
            call(json!([image("source-64x48.png")])),
            json!({ CLIENT: "empty-1" }),
        )
        .unwrap()
        .structured
        .unwrap();
    assert_ne!(accepted["status"], "invalid", "{accepted}");
    assert_eq!(accepted["run"]["client_run_id"], "empty-1");
}
