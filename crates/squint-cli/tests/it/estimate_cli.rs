//! Task 7.6, then 9.4: `squint estimate --json` prints the contract's estimate of one check
//! (table-manners 4.5), the document the served tool returns with no session, priced from the
//! catalog with no credential and writing nothing; `--max-spend-usd` lowers the cap and `check`
//! then refuses before any request.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn squint(args: &[&str]) -> Output {
    squint_with(args, None)
}

/// `squint` with ANTHROPIC_API_KEY set to `key`, or unset.
fn squint_with(args: &[&str], key: Option<&str>) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_squint"));
    c.current_dir(repo_root())
        .args(args)
        .env("SQUINT_ANTHROPIC_ACCOUNT", "org-estimate");
    match key {
        Some(k) => c.env("ANTHROPIC_API_KEY", k),
        None => c.env_remove("ANTHROPIC_API_KEY"),
    };
    c.output().unwrap()
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

/// The estimate printed, valid against `estimate.json`.
fn json(o: &Output) -> serde_json::Value {
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let doc = serde_json::from_slice(&o.stdout).unwrap();
    valid("urn:table-manners:core:estimate", &doc);
    doc
}

const PAIR: [&str; 7] = [
    "--config",
    "examples/source-fidelity.toml",
    "--use-case",
    "source-fidelity",
    "--candidate",
    "fixtures/images/candidate-64x48.png",
    "--source",
];

fn pair(extra: &[&str]) -> Vec<String> {
    let mut args: Vec<String> = ["estimate", "--json"]
        .iter()
        .chain(PAIR.iter())
        .map(|s| s.to_string())
        .collect();
    args.push("fixtures/images/source-64x48.png".into());
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

fn strs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

/// A real profile is priced from the catalog's prices with no key read: unset, the estimate is
/// refused for readiness, still priced. A command-line estimate has no session: `session_id` and
/// `session` are null, so no server accepts its binding.
#[test]
fn a_real_profile_is_estimated_without_a_credential() {
    let e = json(&squint(&strs(&pair(&[]))));
    assert_eq!(e["action"], "source_fidelity");
    assert!(e["worst_case_usd"].as_f64().unwrap() > 0.0, "{e}");
    assert_eq!(e["effective"]["model"], "claude-haiku-5-5");
    assert!(
        e["effective"]["config_identity"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    assert_eq!(e["session_id"], serde_json::Value::Null);
    assert_eq!(e["session"], serde_json::Value::Null);
    assert_eq!(e["refused"], "readiness");
    assert_eq!(e["readiness"]["state"], "needs_setup");
    // The same call estimated again binds the same; with a key set, it is allowed.
    let ready = json(&squint_with(
        &strs(&pair(&[])),
        Some("sk-estimate-not-a-key"),
    ));
    assert_eq!(ready["binding"], e["binding"]);
    assert_eq!(ready["allowed"], true, "{ready}");
    assert!(
        !String::from_utf8_lossy(
            &squint_with(&strs(&pair(&[])), Some("sk-estimate-not-a-key")).stdout
        )
        .contains("sk-estimate-not-a-key")
    );
}

/// `--max-spend-usd` lowers the cap; below the worst case the estimate is refused `cap`. One at or
/// below zero, or above the configured cap, is a usage error (exit 64), never silently lowered.
#[test]
fn a_lower_cap_is_reported_and_a_bad_one_is_a_usage_error() {
    let key = Some("sk-estimate-not-a-key");
    let e = json(&squint_with(
        &strs(&pair(&["--max-spend-usd", "0.000001"])),
        key,
    ));
    assert_eq!(e["cap_usd"], 0.000001);
    assert_eq!(
        (e["allowed"].clone(), e["refused"].clone()),
        (serde_json::json!(false), serde_json::json!("cap"))
    );
    for bad in ["0", "5"] {
        let o = squint_with(&strs(&pair(&["--max-spend-usd", bad])), key);
        assert_eq!(
            o.status.code(),
            Some(64),
            "{bad}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(o.stdout.is_empty());
    }
}

/// `--json` is the only form.
#[test]
fn estimate_requires_json() {
    let mut args = pair(&[]);
    args.retain(|a| a != "--json");
    assert_eq!(squint(&strs(&args)).status.code(), Some(64));
}

/// A check whose cap the call lowered below its envelope refuses before any request, and its
/// `run.json` says `budget`.
#[cfg(feature = "testkit")]
#[test]
fn a_check_under_a_lowered_cap_refuses_before_any_request() {
    let out = std::env::temp_dir().join(format!("squint-estimate-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let o = squint(&[
        "check",
        "--config",
        "fixtures/config/minimal.toml",
        "--job",
        "fixtures/jobs/pair.toml",
        "--provider-script",
        "fixtures/scripts/held.json",
        "--max-spend-usd",
        "0.000001",
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(
        o.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let dir = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let run: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(run["status"], "error");
    assert_eq!(run["error"]["code"], "budget");
    assert_eq!(run["spend"]["usd"], 0.0);
}

/// The scripted provider prices its own requests, so a testkit estimate matches what the fake run
/// would reserve.
#[cfg(feature = "testkit")]
#[test]
fn the_scripted_provider_is_estimated_from_its_own_prices() {
    let e = json(&squint(&[
        "estimate",
        "--json",
        "--config",
        "fixtures/config/minimal.toml",
        "--job",
        "fixtures/jobs/pair.toml",
        "--provider-script",
        "fixtures/scripts/held.json",
    ]));
    assert!(e["worst_case_usd"].as_f64().unwrap() > 0.0);
    assert_eq!(e["allowed"], true, "{e}");
    assert_eq!(e["effective"]["x-squint"]["decision_runs"], 1);
}
