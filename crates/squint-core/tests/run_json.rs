//! Task 7.4: `run.json`, the contract's result (table-manners 2 and 11.1), written last and
//! atomically into every decision directory: valid against the contract's schema for every
//! terminal state, with squint's errors mapped to the contract's status and codes.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::run;
use squint_core::tool::{Envelope, Tool, ToolRegistry};
use squint_testkit::{ScriptedProvider, ScriptedTool};
use table_manners::images::LocationKind;
use table_manners::{ErrorCode, RunResult, Status};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-runjson-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(overrides: &[&str]) -> Config {
    let text = std::fs::read_to_string(root().join("fixtures/config/fake-tool.toml")).unwrap();
    let overrides: Vec<String> = overrides.iter().map(|s| s.to_string()).collect();
    Config::parse(&text, &overrides, &|_| None).unwrap()
}

fn tools() -> ToolRegistry {
    let text = std::fs::read_to_string(root().join("fixtures/tool-scripts/empty.json")).unwrap();
    let envelopes: Vec<Envelope> = serde_json::from_str(&text).unwrap();
    let t: Vec<Box<dyn Tool>> = vec![Box::new(ScriptedTool::fake_regions(envelopes))];
    ToolRegistry::new(t, &["fake_regions".into()], &[]).unwrap()
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

/// Runs `job` against `script` and returns the outcome and the parsed, schema-checked `run.json`.
fn run(
    tag: &str,
    script: &str,
    job: &str,
    overrides: &[&str],
    cancel: Option<&run::Cancel>,
) -> (run::Outcome, RunResult) {
    let out = tempdir(tag);
    let provider =
        ScriptedProvider::from_path(&root().join("fixtures/scripts").join(script)).unwrap();
    let tools = tools();
    let job = Job::load(&root().join("fixtures/jobs").join(job)).unwrap();
    let mut services = run::Services::new(&provider, &tools);
    services.cancel = cancel;
    let outcome = block_on(run::check(
        &config(overrides),
        &job,
        &root(),
        &services,
        &out,
        "test",
    ))
    .unwrap();
    let result = read(&outcome.dir);
    (outcome, result)
}

fn read(dir: &Path) -> RunResult {
    let text = std::fs::read_to_string(dir.join("run.json")).unwrap();
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    valid(&doc);
    serde_json::from_value(doc).unwrap()
}

fn valid(doc: &serde_json::Value) {
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
        .find(|(id, _)| *id == "urn:table-manners:core:result")
        .unwrap();
    let schema: serde_json::Value = serde_json::from_str(text).unwrap();
    let validator = jsonschema::options()
        .with_retriever(Embedded)
        .build(&schema)
        .unwrap();
    let errors: Vec<String> = validator.iter_errors(doc).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{errors:?}\n{doc:#}");
}

#[test]
fn a_pass_is_completed_with_the_header_and_squints_body() {
    let (o, r) = run("pass", "held.json", "pair.toml", &[], None);
    assert_eq!(o.exit_code, 0);
    assert_eq!(r.status, Status::Completed);
    assert!(r.error.is_none());
    assert_eq!(r.contract, table_manners::CONTRACT_VERSION);
    assert_eq!(
        (r.tool.name.as_str(), r.tool.version.as_str()),
        ("squint", "test")
    );
    // The action is the use case's, as `describe` names it, on every surface.
    assert_eq!(r.action, "source_fidelity");
    let id = o.dir.file_name().unwrap().to_str().unwrap();
    assert_eq!(r.run.id, id);
    assert_eq!(r.run.resource, format!("squint://run/{id}/run.json"));
    assert_eq!(r.run.dir.as_deref(), Some(o.dir.to_str().unwrap()));
    assert!(r.input_identity.starts_with("sha256:"));
    assert_eq!(r.spend.basis, table_manners::SpendBasis::Measured);
    assert!(r.spend.usd >= 0.0);
    let images = r.images.unwrap();
    let roles: Vec<(&str, &str)> = images
        .taken
        .iter()
        .map(|i| (i.slot.as_str(), i.role.as_str()))
        .collect();
    assert_eq!(roles, [("candidate", "candidate"), ("source", "source")]);
    assert!(images.out.is_empty());
    assert_eq!(r.body["verdict"], "pass");
    assert_eq!(
        r.body["report"].as_str(),
        Some(o.dir.join("report.json").to_str().unwrap())
    );
}

#[test]
fn a_fail_verdict_is_completed_and_carries_the_finding_as_a_contract_location() {
    let (o, r) = run("fail", "finding-violation.json", "pair.toml", &[], None);
    assert_eq!(o.exit_code, 1);
    assert_eq!(r.status, Status::Completed);
    assert_eq!(r.body["verdict"], "fail");
    let found = r.locations.unwrap().found;
    assert!(!found.is_empty());
    for loc in &found {
        assert_ne!(loc.kind, LocationKind::View);
        let source = loc.source.as_ref().unwrap();
        assert_eq!(source.run.as_deref(), Some(r.run.id.as_str()));
        assert_eq!(source.item.as_deref(), Some("SF1"));
    }
}

#[test]
fn an_exhausted_provider_is_a_retryable_provider_error() {
    let (o, r) = run(
        "provider",
        "retries-exhausted.json",
        "pair.toml",
        &["budget.retries=2"],
        None,
    );
    assert_eq!(o.exit_code, 3);
    assert_eq!(r.status, Status::Error);
    let e = r.error.unwrap();
    assert_eq!((e.code, e.retryable), (ErrorCode::Provider, true));
    assert!(!e.reason.is_empty());
}

#[test]
fn a_job_that_does_not_resolve_is_invalid_usage_and_exits_64() {
    let (o, r) = run("resolve", "held.json", "oversized.toml", &[], None);
    assert_eq!(o.exit_code, 64);
    assert_eq!(r.status, Status::Invalid);
    let e = r.error.unwrap();
    assert_eq!((e.code, e.retryable), (ErrorCode::Usage, false));
    assert_eq!(r.spend.usd, 0.0);
}

#[test]
fn an_image_that_is_not_the_expected_one_is_invalid_input() {
    let (o, r) = run("input", "held.json", "expect-mismatch.toml", &[], None);
    assert_eq!(o.exit_code, 64);
    assert_eq!(r.status, Status::Invalid);
    assert_eq!(r.error.unwrap().code, ErrorCode::Input);
}

#[test]
fn a_cancelled_decision_is_cancelled_with_its_spend_to_date() {
    let cancel = run::Cancel::new();
    cancel.cancel();
    let (_, r) = run("cancelled", "held.json", "pair.toml", &[], Some(&cancel));
    assert_eq!(r.status, Status::Cancelled);
    assert!(r.error.is_none());
    assert!(r.spend.usd >= 0.0);
}

/// `run.json` is written to a temporary name and renamed, so a reader never sees half of one and
/// nothing is left behind.
#[test]
fn run_json_is_written_atomically_and_leaves_no_temporary_file() {
    let (o, _) = run("atomic", "held.json", "pair.toml", &[], None);
    let names: Vec<String> = std::fs::read_dir(&o.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n == "run.json"));
    assert!(
        names.iter().all(|n| !n.starts_with(".run.json")),
        "a temporary file was left: {names:?}"
    );
}

#[test]
fn timestamps_are_rfc3339_in_utc() {
    use squint_core::contract::rfc3339;
    assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
    assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
    assert_eq!(rfc3339(1_793_386_884_140), "2026-10-30T19:01:24.140Z");
    assert_eq!(rfc3339(4_102_444_799_999), "2099-12-31T23:59:59.999Z");
}
