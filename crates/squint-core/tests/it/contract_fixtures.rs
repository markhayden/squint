//! Tasks 7.10 and 9.10: squint's conformance fixtures, the documents table-manners keeps under
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
use squint_core::contract::Executor;
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
    let mut services = run::Services::new(&provider, &tools);
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
        // The served documents and the command-line estimate are squint-cli's to generate
        // (contract_fixtures_served.rs).
        [
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
            "refusal-invalid-arguments.json",
            "refusal-no-run.json",
            "refusal-not-ready.json",
            "run-accepted-after-refusal.json",
            "run-budget.json",
            "run-cancelled.json",
            "run-cap.json",
            "run-completed.json",
            "run-credential.json",
            "run-error.json",
            "run-fail.json",
            "run-invalid.json",
            "run-served-cancelled-unresolved.json",
            "run-stale-estimate-argument.json",
            "run-stale-estimate-expired.json",
            "run-stale-estimate-image.json",
            "run-stale-estimate-session.json",
            "runs.json",
            "session.json",
        ],
        "a fixture squint no longer produces"
    );
}

/// The fixtures as table-manners' `tests/fixtures.rs` reads them, so the copy into its
/// `fixtures/squint/` passes there first time: the schema chosen by file name (`describe*.json`
/// a description, `estimate-*.json` an estimate, `run-*.json` a result, `runs*.json` a runs page,
/// `session.json` a session, `refusal-*.json` refusal data), validated with the crate's embedded
/// schemas; contract `0.3.0` and tool `squint` wherever a document names them; and every amount
/// (`usd` or a key ending `_usd`) at most six decimals.
#[test]
fn every_fixture_validates_as_table_manners_reads_it() {
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
    fn schema_for(name: &str) -> Option<&'static str> {
        Some(if name.starts_with("describe") {
            "urn:table-manners:core:description"
        } else if name.starts_with("estimate-") {
            "urn:table-manners:core:estimate"
        } else if name.starts_with("run-") {
            "urn:table-manners:core:result"
        } else if name.starts_with("runs") {
            "urn:table-manners:core:runs"
        } else if name == "session.json" {
            "urn:table-manners:core:session"
        } else if name.starts_with("refusal-") {
            "urn:table-manners:core:refusal"
        } else {
            return None;
        })
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
    assert_eq!(table_manners::CONTRACT_VERSION, "0.3.0");
    let mut seen = 0;
    for entry in std::fs::read_dir(dir()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let schema_id = schema_for(&name).unwrap_or_else(|| panic!("{name}: no schema by name"));
        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let (_, text) = table_manners::SCHEMAS
            .iter()
            .find(|(id, _)| *id == schema_id)
            .unwrap();
        let schema: Value = serde_json::from_str(text).unwrap();
        let validator = jsonschema::options()
            .with_retriever(Embedded)
            .build(&schema)
            .unwrap();
        let errors: Vec<String> = validator.iter_errors(&doc).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{name} ({schema_id}): {errors:?}");
        if let Some(contract) = doc.get("contract") {
            assert_eq!(contract, "0.3.0", "{name}");
        }
        if let Some(tool) = doc.get("tool") {
            assert_eq!(tool["name"], "squint", "{name}");
        }
        let mut bad = Vec::new();
        amounts(&doc, "", "", &mut bad);
        assert!(bad.is_empty(), "{name}: more than six decimals: {bad:?}");
        seen += 1;
    }
    assert!(seen >= 34, "only {seen} fixtures");
}
