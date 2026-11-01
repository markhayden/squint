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
    assert_eq!(r.spend.usd, 0.0, "cancelled before the first request");
}

/// A decision cancelled after it has spent keeps that spend in its `run.json` (table-manners
/// 2.3): the first turn is paid for, the cancel lands during a 30 s retry wait, promptly.
#[test]
fn a_decision_cancelled_after_spending_keeps_its_spend() {
    let cancel = run::Cancel::new();
    let trigger = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(500));
        trigger.cancel();
    });
    let started = std::time::Instant::now();
    let (_, r) = run(
        "cancelled-spent",
        "spend-then-slow-retry.json",
        "pair.toml",
        &[],
        Some(&cancel),
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert_eq!(r.status, Status::Cancelled, "{:?}", r.error);
    assert!(r.spend.usd > 0.0, "{:?}", r.spend);
}

/// A decision whose cache cannot take it still succeeded (review I2): the store's failure is a
/// note on a `completed` run, and the decision's verdict and spend stay in its own `run.json`.
#[test]
fn a_cache_that_cannot_store_is_a_note_not_a_lost_decision() {
    use std::os::unix::fs::PermissionsExt;
    let cache = tempdir("cache-readonly");
    std::fs::create_dir_all(cache.join("decisions")).unwrap();
    std::fs::set_permissions(
        cache.join("decisions"),
        std::fs::Permissions::from_mode(0o555),
    )
    .unwrap();
    let dir_override = format!("cache.dir={:?}", cache.display().to_string());
    let (o, r) = run(
        "cache-note",
        "held.json",
        "pair.toml",
        &[&dir_override],
        None,
    );
    std::fs::set_permissions(
        cache.join("decisions"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert_eq!(o.exit_code, 0);
    assert_eq!(r.status, Status::Completed);
    assert!(r.notes.iter().any(|n| n.contains("cache")), "{:?}", r.notes);
}

/// An error after the decision directory exists still ends that directory with its own
/// `run.json` (review I2), and the services are told the directory when it is made, so a server
/// answers with that run rather than inventing another.
#[test]
fn a_decision_that_fails_after_allocation_keeps_its_own_run_json() {
    let out = tempdir("late-error");
    let not_a_dir = out.join("cache-is-a-file");
    std::fs::write(&not_a_dir, "").unwrap();
    let provider = ScriptedProvider::from_path(&root().join("fixtures/scripts/held.json")).unwrap();
    let tools = tools();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let told = std::sync::Arc::new(std::sync::Mutex::new(None::<PathBuf>));
    let seen = told.clone();
    let services = run::Services::new(&provider, &tools).with_allocated(std::sync::Arc::new(
        move |dir: &Path| *seen.lock().unwrap() = Some(dir.to_path_buf()),
    ));
    let dir_override = format!("cache.dir={:?}", not_a_dir.display().to_string());
    let err = block_on(run::check(
        &config(&[&dir_override]),
        &job,
        &root(),
        &services,
        &out.join("decisions"),
        "test",
    ))
    .unwrap_err();
    let dir = told
        .lock()
        .unwrap()
        .clone()
        .expect("told of the decision directory");
    let r = read(&dir);
    assert_eq!(r.status, Status::Error, "{err}");
    assert_eq!(r.error.unwrap().code, ErrorCode::Io);
    assert_eq!(
        std::fs::read_dir(out.join("decisions")).unwrap().count(),
        1,
        "one decision, one directory"
    );
}

/// A reference comes in through the `references` slot, the one `describe` declares, with the
/// role `reference` (review I3; table-manners 11.1).
#[test]
fn a_reference_is_taken_in_through_the_references_slot() {
    let (_, r) = run("references", "held.json", "pip.toml", &[], None);
    let taken = &r.images.as_ref().unwrap().taken;
    let reference = taken.iter().find(|i| i.role == "reference").unwrap();
    assert_eq!(reference.slot, "references");
    let candidate = taken.iter().find(|i| i.role == "candidate").unwrap();
    assert_eq!(candidate.slot, "candidate");
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
