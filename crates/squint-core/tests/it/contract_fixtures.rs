//! Tasks 7.10, 9.10 and 10.6: squint's conformance fixtures, the documents table-manners keeps under
//! `fixtures/squint/` (CONTRACT.md 7 item 7): the descriptions (unconfigured, each credential
//! state, a ready action) and one `run.json` per status, generated from squint's own code with the
//! volatile fields (times, run ids, directories, versions) fixed. The committed copy under
//! `fixtures/contract/squint/` must equal what squint produces now, so a drifting field fails
//! here; `UPDATE_CONTRACT_FIXTURES=1` rewrites it after a deliberate change, and the copy then goes
//! to table-manners. The served documents are squint-cli's (`contract_fixtures_served.rs`); this
//! file also checks the whole directory as table-manners' own test reads it.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use serde_json::Value;
use squint_core::config::{Config, ConfigFile, ConfigState};
use squint_core::contract::{CallFacts, Executor};
use squint_core::job::Job;
use squint_core::readiness::{Answer, Answers, Credential, Inspection, config_requirement};
use squint_core::run;
use squint_core::tool::{Envelope, Tool, ToolRegistry};
use squint_testkit::{ScriptedProvider, ScriptedTool};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn dir() -> PathBuf {
    root().join("fixtures/contract/squint")
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

/// One decision's `run.json`, its volatile fields fixed under the name `case`.
fn run_case(case: &str, script: &str, job: &str, overrides: &[&str], cancelled: bool) -> Value {
    let out = std::env::temp_dir().join(format!("squint-fixture-{case}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let text = std::fs::read_to_string(root().join("fixtures/config/fake-tool.toml")).unwrap();
    let overrides: Vec<String> = overrides.iter().map(|s| s.to_string()).collect();
    let config = Config::parse(&text, &overrides, &|_| None).unwrap();
    let provider =
        ScriptedProvider::from_path(&root().join("fixtures/scripts").join(script)).unwrap();
    let envelopes: Vec<Envelope> = serde_json::from_str(
        &std::fs::read_to_string(root().join("fixtures/tool-scripts/empty.json")).unwrap(),
    )
    .unwrap();
    let tools: Vec<Box<dyn Tool>> = vec![Box::new(ScriptedTool::fake_regions(envelopes))];
    let tools = ToolRegistry::new(tools, &["fake_regions".into()], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs").join(job)).unwrap();
    let cancel = run::Cancel::new();
    // What `squint check --config fixtures/config/fake-tool.toml` names in the invocation: the
    // fake profile reads no credential and calls no catalog model, so the configuration alone.
    let mut services = run::Services::new(&provider, &tools).with_call(CallFacts {
        config_identity: None,
        max_spend_usd: None,
        requirements: vec!["fake-tool.toml".into()],
    });
    if cancelled {
        // Cancelled once the decision is allocated and reserved, before its first request.
        let trigger = cancel.clone();
        services = services
            .with_cancel(&cancel)
            .with_observer(std::sync::Arc::new(move |_: &Path, event: &str| {
                if event == "reserved" {
                    trigger.cancel();
                }
            }));
    }
    let outcome = block_on(run::check(&config, &job, &root(), &services, &out, "0.0.0")).unwrap();
    let mut result: Value =
        serde_json::from_str(&std::fs::read_to_string(outcome.dir.join("run.json")).unwrap())
            .unwrap();
    fix(&mut result, case);
    result
}

/// Times, run ids, directories and versions are the run's, not squint's: fixed so the fixture
/// changes only when a field squint emits changes.
fn fix(result: &mut Value, case: &str) {
    result["started"] = "2026-01-01T00:00:00.000Z".into();
    result["finished"] = "2026-01-01T00:00:00.000Z".into();
    result["duration_ms"] = 0.into();
    result["run"]["id"] = case.into();
    result["run"]["resource"] = format!("squint://run/{case}/run.json").into();
    result["run"]["dir"] = format!("/runs/{case}").into();
    if result["body"].get("report").is_some() {
        result["body"]["report"] = format!("/runs/{case}/report.json").into();
    }
    for location in result["locations"]["found"]
        .as_array_mut()
        .into_iter()
        .flatten()
    {
        location["source"]["run"] = case.into();
        location["source"]["version"] = "0.0.0".into();
    }
    if let Some(error) = result.get_mut("error").filter(|e| !e.is_null()) {
        let reason = error["reason"].as_str().unwrap_or_default().to_string();
        error["reason"] = reason
            .replace(
                &root().canonicalize().unwrap().display().to_string(),
                "<repo>",
            )
            .into();
    }
}

/// The description of the server a host runs for every use case on the shipped example: a
/// config without `[rules]` (one that names them describes only its own use case), its
/// credential set (`present`), unset (`missing`), or settled by this session's answers (`valid`
/// after an accepted request, `invalid` after a 401 or 403).
fn description(credential: Option<Answer>, present: bool) -> Value {
    let text = std::fs::read_to_string(root().join("examples/source-fidelity.toml")).unwrap();
    let mut config = Config::parse(&text, &[], &|_| Some("org-fixture".into())).unwrap();
    config.rules = None;
    let answers = Answers::default();
    if let Some(answer) = credential {
        answers.record(Some("ANTHROPIC_API_KEY"), &config.provider.model, answer);
    }
    let inspection = Inspection {
        config: present_config(),
        credential: Some(Credential {
            name: "ANTHROPIC_API_KEY".to_string(),
            present,
        }),
        model: Some(config.provider.model.clone()),
        files: Vec::new(),
    };
    describe(Some(&config), Executor::Paid, &inspection, &answers)
}

fn present_config() -> table_manners::Requirement {
    table_manners::Requirement {
        kind: table_manners::RequirementKind::Config,
        name: "squint.toml".into(),
        state: table_manners::RequirementState::Present,
        setup: None,
    }
}

fn describe(
    config: Option<&Config>,
    executor: Executor,
    inspection: &Inspection,
    answers: &Answers,
) -> Value {
    serde_json::to_value(
        squint_core::contract::description(
            config,
            "0.0.0",
            executor,
            &inspection.readiness(answers),
        )
        .unwrap(),
    )
    .unwrap()
}

/// With no configuration anywhere on the lookup: every shipped use case, the default limits,
/// and only the `config` requirement, `missing` at the path the lookup would use.
fn unconfigured() -> Value {
    let file = ConfigFile {
        path: PathBuf::from("/home/user/.config/squint/squint.toml"),
        state: ConfigState::Missing,
    };
    let inspection = Inspection::config_only(config_requirement(&file));
    describe(None, Executor::Paid, &inspection, &Answers::default())
}

/// A ready action with nothing to set up: the scripted fake profile reads no credential and
/// calls no catalog model, so its checks are free and only the configuration is listed.
fn ready() -> Value {
    let text = std::fs::read_to_string(root().join("fixtures/config/minimal.toml")).unwrap();
    let config = Config::parse(&text, &[], &|_| None).unwrap();
    let inspection = Inspection::config_only(present_config());
    let doc = describe(
        Some(&config),
        Executor::Free,
        &inspection,
        &Answers::default(),
    );
    assert!(
        doc["actions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|a| a["readiness"]["state"] == "ready"),
        "{doc}"
    );
    doc
}

fn check(name: &str, now: &Value) {
    let path = dir().join(name);
    let text = serde_json::to_string_pretty(now).unwrap() + "\n";
    if std::env::var_os("UPDATE_CONTRACT_FIXTURES").is_some() {
        std::fs::create_dir_all(dir()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        committed,
        text,
        "{} drifted from what squint emits; review, then UPDATE_CONTRACT_FIXTURES=1 and copy to table-manners",
        path.strip_prefix(Path::new(&root()))
            .unwrap_or(&path)
            .display()
    );
}

#[test]
fn the_conformance_fixtures_are_what_squint_emits() {
    check("describe.json", &description(None, true));
    check("describe-unconfigured.json", &unconfigured());
    check(
        "describe-credential-missing.json",
        &description(None, false),
    );
    check("describe-credential-present.json", &description(None, true));
    check(
        "describe-credential-valid.json",
        &description(Some(Answer::Accepted), true),
    );
    check(
        "describe-credential-invalid.json",
        &description(Some(Answer::Rejected), true),
    );
    check("describe-ready.json", &ready());
    check(
        "run-completed.json",
        &run_case("completed", "held.json", "pair.toml", &[], false),
    );
    check(
        "run-fail.json",
        &run_case("fail", "finding-violation.json", "pair.toml", &[], false),
    );
    check(
        "run-invalid.json",
        &run_case("invalid", "held.json", "expect-mismatch.toml", &[], false),
    );
    check(
        "run-error.json",
        &run_case(
            "error",
            "retries-exhausted.json",
            "pair.toml",
            &["budget.retries=2"],
            false,
        ),
    );
    check(
        "run-cancelled.json",
        &run_case("cancelled", "held.json", "pair.toml", &[], true),
    );
    let mut files: Vec<String> = std::fs::read_dir(dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(
        files,
        // The served documents, the command-line estimate, the call, the bundle and its replay
        // are squint-cli's to generate (contract_fixtures_served.rs).
        [
            "bundle-check",
            "describe-credential-invalid.json",
            "describe-credential-missing.json",
            "describe-credential-present.json",
            "describe-credential-valid.json",
            "describe-ready.json",
            "describe-unconfigured.json",
            "describe.json",
            "estimate-allowed.json",
            "estimate-cli.json",
            "estimate-refused-budget.json",
            "estimate-refused-cap.json",
            "estimate-refused-readiness-unconfigured.json",
            "estimate-refused-readiness.json",
            "refusal-cancelled-before-allocation.json",
            "refusal-earlier-run.json",
            "refusal-invalid-arguments-path-outside.json",
            "refusal-invalid-arguments.json",
            "refusal-no-run.json",
            "refusal-not-ready.json",
            "run-accepted-after-refusal.json",
            "run-budget.json",
            "run-bundle-replay.json",
            "run-call.json",
            "run-cancelled.json",
            "run-cap.json",
            "run-completed.json",
            "run-credential.json",
            "run-error.json",
            "run-fail.json",
            "run-invalid.json",
            "run-path-dotdot.json",
            "run-path-missing.json",
            "run-path-no-allow-dir.json",
            "run-path-outside.json",
            "run-path-symlink.json",
            "run-served-cancelled-unresolved.json",
            "run-stale-estimate-argument.json",
            "run-stale-estimate-expired.json",
            "run-stale-estimate-image.json",
            "run-stale-estimate-session.json",
            "runs.json",
            "session-narrowed.json",
            "session-no-allow-dir.json",
            "session.json",
        ],
        "a fixture squint no longer produces"
    );
}

/// The fixtures as table-manners' `tests/fixtures.rs` reads them (its `v0.4` branch), so the copy
/// into its `fixtures/squint/` passes there first time. Each file by its name: `describe*.json` a
/// description, `estimate-*.json` an estimate, `run-*.json` a result, `runs*.json` a runs page,
/// `session.json` and `session-*.json` a session, `refusal-*.json` refusal data; each validated
/// with the crate's embedded schemas and read into the crate's type; every `bundle-*/` directory
/// an invocation bundle whose `invocation.json` is a manifest and whose every listed member is
/// beside it with the digest the manifest states; any other file or directory fails. Every
/// contract version in a document (a bundle's steps and results too) is `0.4.0`, every tool
/// `squint`, every amount (`usd` or a key ending `_usd`) at most six decimals. Beside the schemas,
/// the rules they cannot say, as table-manners checks them: in a result and in each bundled step,
/// `invocation.action` is the result's action and no image in the invocation has a locator (4.8).
/// And squint's own v0.4 promises: every action of kind `task` with no `group`, no
/// `artifact_types` and no `ports`; no result with `artifacts`; `invocation` on every result,
/// null only on an `invalid` one; requirement names that are names, never paths (no `/` or `\`,
/// 4.8); no operations-profile file (`run-op-*`, `refusal-binding-*`, `artifact-*`, `export-*`,
/// `import-*`), since squint lists only `images`.
#[test]
fn every_fixture_validates_as_table_manners_reads_it() {
    use sha2::{Digest, Sha256};
    use table_manners::{
        Description, Estimate, InvocationBundle, Refusal, RunResult, RunsList, Session,
    };

    struct Embedded;
    impl jsonschema::Retrieve for Embedded {
        fn retrieve(
            &self,
            uri: &jsonschema::Uri<String>,
        ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
            let (_, text) = table_manners::SCHEMAS
                .iter()
                .find(|(id, _)| *id == uri.as_str())
                .ok_or("unknown schema")?;
            Ok(serde_json::from_str(text)?)
        }
    }
    fn valid(schema_id: &str, name: &str, doc: &Value) {
        let (_, text) = table_manners::SCHEMAS
            .iter()
            .find(|(id, _)| *id == schema_id)
            .unwrap();
        let schema: Value = serde_json::from_str(text).unwrap();
        let validator = jsonschema::options()
            .with_retriever(Embedded)
            .build(&schema)
            .unwrap();
        let errors: Vec<String> = validator.iter_errors(doc).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{name} ({schema_id}): {errors:?}");
    }
    fn amounts(v: &Value, key: &str, at: &str, bad: &mut Vec<String>) {
        match v {
            Value::Object(map) => {
                for (k, value) in map {
                    amounts(value, k, &format!("{at}/{k}"), bad);
                }
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    amounts(item, key, &format!("{at}/{i}"), bad);
                }
            }
            Value::Number(n) if key == "usd" || key.ends_with("_usd") => {
                let x = n.as_f64().unwrap();
                let micro = x * 1e6;
                if (micro - micro.round()).abs() > 1e-6 {
                    bad.push(format!("{at} = {n}"));
                }
            }
            _ => {}
        }
    }
    /// Every `contract` in the document is 0.4.0 and every `tool.name` squint.
    fn ours(name: &str, v: &Value) {
        match v {
            Value::Object(map) => {
                for (k, value) in map {
                    match k.as_str() {
                        "contract" => assert_eq!(value, "0.4.0", "{name}"),
                        "tool" if value.get("name").is_some() => {
                            assert_eq!(value["name"], "squint", "{name}");
                        }
                        _ => ours(name, value),
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|i| ours(name, i)),
            _ => {}
        }
    }
    /// No image inside an invocation's arguments has a locator (4.8): no object holds `inline`
    /// bytes, and no object of only `path` and `expect` holds a `path`; nor any delivery-only
    /// argument.
    fn no_locator(name: &str, v: &Value) {
        match v {
            Value::Object(map) => {
                let inline = map
                    .get("inline")
                    .is_some_and(|i| i.get("media_type").is_some());
                let by_path =
                    map.contains_key("path") && map.keys().all(|k| k == "path" || k == "expect");
                assert!(!inline && !by_path, "{name}: an image with a locator");
                for k in ["return_images", "image_blocks"] {
                    assert!(!map.contains_key(k), "{name}: the invocation holds {k}");
                }
                map.values().for_each(|value| no_locator(name, value));
            }
            Value::Array(items) => items.iter().for_each(|i| no_locator(name, i)),
            _ => {}
        }
    }
    /// A result's rules (4.8 and squint's v0.4 promises), on its raw document and its type.
    fn result_rules(name: &str, doc: &Value) {
        let r: RunResult =
            serde_json::from_value(doc.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(doc.get("artifacts").is_none(), "{name}: artifacts");
        let invocation = doc
            .get("invocation")
            .unwrap_or_else(|| panic!("{name}: no invocation"));
        match &r.invocation {
            None => assert_eq!(doc["status"], "invalid", "{name}: invocation null"),
            Some(i) => {
                assert_eq!(
                    i.action, r.action,
                    "{name}: the invocation is of another action"
                );
                no_locator(name, &invocation["arguments"]);
                for requirement in &i.requirements {
                    assert!(
                        !requirement.contains(['/', '\\']),
                        "{name}: requirement {requirement:?} is a path, not a name"
                    );
                }
            }
        }
    }
    fn sha256_hex(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
    /// A `bundle-*/` directory: `invocation.json` a manifest; every step's invocation of its
    /// result's action, its result held to the result rules; every listed member beside the
    /// manifest with the digest it states, and nothing else beside it.
    fn bundle(dir: &Path) -> usize {
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        let path = dir.join("invocation.json");
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{name}/invocation.json: {e}"));
        let mut bad = Vec::new();
        amounts(&doc, "", "", &mut bad);
        assert!(bad.is_empty(), "{name}: more than six decimals: {bad:?}");
        ours(&name, &doc);
        valid("urn:table-manners:core:invocation-bundle", &name, &doc);
        let b: InvocationBundle =
            serde_json::from_value(doc.clone()).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut listed = vec!["invocation.json".to_string()];
        for (i, step) in b.steps.iter().enumerate() {
            assert_eq!(
                step.invocation.action, step.result.action,
                "{name}: steps[{i}]'s invocation is of another action"
            );
            result_rules(
                &format!("{name}: steps[{i}].result"),
                &doc["steps"][i]["result"],
            );
            assert!(
                step.artifacts.is_empty()
                    && step.unportable.is_empty()
                    && step.reference_only.is_empty(),
                "{name}: steps[{i}] has what squint never replays"
            );
            for input in &step.inputs {
                let bytes = std::fs::read(dir.join(&input.member))
                    .unwrap_or_else(|e| panic!("{name}: member {}: {e}", input.member));
                assert_eq!(
                    sha256_hex(&bytes),
                    input.image.file_sha256,
                    "{name}: member {} is not the bytes the manifest names",
                    input.member
                );
                listed.push(input.member.clone());
            }
        }
        let mut found = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(at) = stack.pop() {
            for entry in std::fs::read_dir(&at).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let member = path.strip_prefix(dir).unwrap().to_string_lossy();
                    found.push(member.replace('\\', "/"));
                }
            }
        }
        listed.sort();
        listed.dedup();
        found.sort();
        assert_eq!(found, listed, "{name}: a file the manifest does not list");
        b.steps.len()
    }

    assert_eq!(table_manners::CONTRACT_VERSION, "0.4.0");
    let mut seen = 0;
    let mut results = 0;
    for entry in std::fs::read_dir(dir()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        for prefix in [
            "run-op-",
            "refusal-binding-",
            "artifact-",
            "export-",
            "import-",
        ] {
            assert!(
                !name.starts_with(prefix),
                "{name}: an operations-profile fixture, and squint lists only images"
            );
        }
        if path.is_dir() {
            assert!(
                name.starts_with("bundle-"),
                "{name}: not a fixture table-manners knows"
            );
            assert!(bundle(&path) > 0, "{name}: no step");
            seen += 1;
            continue;
        }
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut bad = Vec::new();
        amounts(&doc, "", "", &mut bad);
        assert!(bad.is_empty(), "{name}: more than six decimals: {bad:?}");
        ours(&name, &doc);
        let parsed = |e: serde_json::Error| -> ! { panic!("{name}: {e}") };
        if name.starts_with("describe") {
            valid("urn:table-manners:core:description", &name, &doc);
            let d: Description = serde_json::from_value(doc.clone()).unwrap_or_else(|e| parsed(e));
            assert_eq!(d.tool.name, "squint", "{name}");
            assert!(d.artifact_types.is_none(), "{name}: artifact_types");
            assert!(
                d.vocabulary.as_ref().is_none_or(|v| v.group.is_none()),
                "{name}: vocabulary.group"
            );
            for a in &d.actions {
                assert_eq!(doc_kind(&doc, &a.name), "task", "{name}: {}", a.name);
                assert!(a.group.is_none(), "{name}: {} has a group", a.name);
                assert!(a.ports.is_none(), "{name}: {} has ports", a.name);
            }
        } else if name.starts_with("estimate-") {
            valid("urn:table-manners:core:estimate", &name, &doc);
            let _: Estimate = serde_json::from_value(doc).unwrap_or_else(|e| parsed(e));
        } else if name.starts_with("run-") {
            valid("urn:table-manners:core:result", &name, &doc);
            result_rules(&name, &doc);
            results += 1;
        } else if name.starts_with("runs") {
            valid("urn:table-manners:core:runs", &name, &doc);
            let _: RunsList = serde_json::from_value(doc).unwrap_or_else(|e| parsed(e));
        } else if name == "session.json" || name.starts_with("session-") {
            valid("urn:table-manners:core:session", &name, &doc);
            let _: Session = serde_json::from_value(doc).unwrap_or_else(|e| parsed(e));
        } else if name.starts_with("refusal-") {
            valid("urn:table-manners:core:refusal", &name, &doc);
            let _: Refusal = serde_json::from_value(doc).unwrap_or_else(|e| parsed(e));
        } else {
            panic!("{name}: not a fixture table-manners knows");
        }
        seen += 1;
    }
    assert!(results > 0, "no run-*.json");
    assert!(seen >= 45, "only {seen} fixtures");
}

/// The `kind` of the action named `action` in a description document.
fn doc_kind<'a>(doc: &'a Value, action: &str) -> &'a str {
    doc["actions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| a["name"] == action)
        .and_then(|a| a["kind"].as_str())
        .unwrap_or_default()
}
