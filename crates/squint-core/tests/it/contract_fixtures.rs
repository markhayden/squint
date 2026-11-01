//! Task 7.10: squint's conformance fixtures, the documents table-manners keeps under
//! `fixtures/squint/`: the description and one `run.json` per status, generated from squint's own
//! code with the volatile fields (times, run ids, directories, versions) fixed. The committed copy
//! under `fixtures/contract/squint/` must equal what squint produces now, so a drifting field fails
//! here; `UPDATE_CONTRACT_FIXTURES=1` rewrites it after a deliberate change, and the copy then goes
//! to table-manners, whose tests validate it against the schemas.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use serde_json::Value;
use squint_core::config::Config;
use squint_core::job::Job;
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
    if cancelled {
        cancel.cancel();
    }
    let mut services = run::Services::new(&provider, &tools);
    if cancelled {
        services = services.with_cancel(&cancel);
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

fn description() -> Value {
    let text = std::fs::read_to_string(root().join("examples/source-fidelity.toml")).unwrap();
    let mut config = Config::parse(&text, &[], &|_| Some("org-fixture".into())).unwrap();
    // The server a host runs for every use case: a config without `[rules]` (one that names
    // them describes only its own use case).
    config.rules = None;
    serde_json::to_value(
        squint_core::contract::description(
            &config,
            "0.0.0",
            &squint_core::contract::Executor::Paid {
                needs_env: vec!["ANTHROPIC_API_KEY".to_string()],
            },
        )
        .unwrap(),
    )
    .unwrap()
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
    check("describe.json", &description());
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
        // The served documents are squint-cli's to generate (contract_fixtures_served.rs).
        [
            "describe.json",
            "refusal-earlier-run.json",
            "refusal-no-run.json",
            "run-cancelled.json",
            "run-completed.json",
            "run-error.json",
            "run-fail.json",
            "run-invalid.json",
            "run-served-cancelled-unresolved.json",
            "run-served-estimate.json",
            "runs.json",
            "session.json",
        ],
        "a fixture squint no longer produces"
    );
}
