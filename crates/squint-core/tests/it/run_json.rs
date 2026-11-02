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
    // A run made outside a server belongs to no session and no client request (table-manners 4.3).
    assert_eq!(
        (r.run.session_id.as_deref(), r.run.client_run_id.as_deref()),
        (None, None)
    );
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

/// The limits check (table-manners 4.5, task 9.5): a cap below the envelope is `cap`, never
/// retryable, with nothing sent; a session ledger that cannot hold it is `budget`.
#[test]
fn a_cap_below_the_envelope_is_cap_and_a_full_session_is_budget() {
    let (o, r) = run(
        "cap",
        "held.json",
        "pair.toml",
        &["budget.spend_cap_usd=0.000001"],
        None,
    );
    assert_eq!(o.exit_code, 3);
    assert_eq!(r.status, Status::Error);
    let e = r.error.unwrap();
    assert_eq!((e.code, e.retryable), (ErrorCode::Cap, false));
    assert_eq!(r.spend.usd, 0.0);

    let out = tempdir("session-budget");
    let provider = ScriptedProvider::from_path(&root().join("fixtures/scripts/held.json")).unwrap();
    let tools = tools();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let session = squint_core::ledger::CapLedger::new(0.000001);
    let services = run::Services::new(&provider, &tools).with_shared_ledger(Some(&session));
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &services,
        &out,
        "test",
    ))
    .unwrap();
    let r = read(&o.dir);
    let e = r.error.unwrap();
    assert_eq!((e.code, e.retryable), (ErrorCode::Budget, false));
    assert_eq!(r.spend.usd, 0.0);
    assert_eq!(provider.remaining(), 1, "nothing was sent");
}

/// A call whose estimate no longer binds it fails `stale_estimate` at the limits check: after
/// its directory exists, before the reservation, with zero spend and nothing sent.
#[test]
fn a_stale_estimate_is_invalid_with_zero_spend() {
    let out = tempdir("stale");
    let provider = ScriptedProvider::from_path(&root().join("fixtures/scripts/held.json")).unwrap();
    let tools = tools();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let services = run::Services::new(&provider, &tools)
        .with_stale_estimate(Some("the estimate expired".into()));
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &services,
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 64);
    let r = read(&o.dir);
    assert_eq!(r.status, Status::Invalid);
    let e = r.error.unwrap();
    assert_eq!((e.code, e.retryable), (ErrorCode::StaleEstimate, false));
    assert!(e.reason.contains("expired"), "{}", e.reason);
    assert_eq!(r.spend.usd, 0.0);
    assert_eq!(provider.remaining(), 1, "nothing was sent");
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
    let (_, r) = scripted(
        "cancelled",
        serde_json::json!([submission()]),
        Some("reserved"),
    );
    assert_eq!(r.status, Status::Cancelled);
    assert!(r.error.is_none());
    assert_eq!(r.spend.usd, 0.0, "cancelled before the first request");
}

/// A decision cancelled after it has spent keeps that spend in its `run.json` (table-manners
/// 2.3): the first turn is paid for, the cancel lands during a 30 s retry wait, promptly.
#[test]
fn a_decision_cancelled_after_spending_keeps_its_spend() {
    let script: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("fixtures/scripts/spend-then-slow-retry.json"))
            .unwrap(),
    )
    .unwrap();
    let started = std::time::Instant::now();
    // Cancelled as the failed second turn starts its 30 s retry wait.
    let (_, r) = scripted("cancelled-spent", script, Some("retry"));
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

/// The bound a turn was sent with, from the run's journal.
fn bound_of(dir: &Path) -> f64 {
    let journal = std::fs::read_to_string(dir.join("run-1/journal.jsonl")).unwrap();
    journal
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .find_map(|e| e["data"]["bound_usd"].as_f64())
        .unwrap()
}

/// Runs `turns` (a script as JSON) with `budget.retries=2`, cancelled as soon as the journal
/// records the event `cancel_on` when given; the outcome's directory and its `run.json`.
fn scripted(
    tag: &str,
    turns: serde_json::Value,
    cancel_on: Option<&'static str>,
) -> (PathBuf, RunResult) {
    let out = tempdir(tag);
    let provider = ScriptedProvider::from_json(&turns.to_string()).unwrap();
    let tools = tools();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let cancel = run::Cancel::new();
    let mut services = run::Services::new(&provider, &tools).with_cancel(&cancel);
    if let Some(event) = cancel_on {
        let trigger = cancel.clone();
        services = services.with_observer(std::sync::Arc::new(move |_: &Path, seen: &str| {
            if seen == event {
                trigger.cancel();
            }
        }));
    }
    let outcome = block_on(run::check(
        &config(&["budget.retries=2"]),
        &job,
        &root(),
        &services,
        &out,
        "test",
    ))
    .unwrap();
    let result = read(&outcome.dir);
    (outcome.dir, result)
}

fn submission() -> serde_json::Value {
    let held: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("fixtures/scripts/held.json")).unwrap(),
    )
    .unwrap();
    held[0].clone()
}

/// Equal as the contract compares amounts: to 1e-6 USD (table-manners 4.6), written amounts being
/// rounded to six decimals.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-6
}

/// A call that never left (connection refused) costs nothing and releases its hold
/// (table-manners 2.3, v0.2): the run spends what a clean run spends, none of it unresolved.
#[test]
fn a_call_that_never_left_costs_nothing() {
    let (_, clean) = scripted("clean", serde_json::json!([submission()]), None);
    let (_, refused) = scripted(
        "not-sent",
        serde_json::json!([{ "error": { "sent": false, "message": "connection refused" } }, submission()]),
        None,
    );
    assert_eq!(refused.status, Status::Completed);
    assert!(
        close(refused.spend.usd, clean.spend.usd),
        "{:?} vs {:?}",
        refused.spend,
        clean.spend
    );
    assert_eq!(refused.spend.unresolved_usd, 0.0);
}

/// An authentication rejection is an answer that refuses before any work: zero, not unresolved.
#[test]
fn an_auth_rejection_costs_nothing() {
    let (_, r) = scripted(
        "auth",
        serde_json::json!([{ "error": { "status": 401 } }]),
        None,
    );
    assert_eq!(r.status, Status::Error);
    assert_eq!((r.spend.usd, r.spend.unresolved_usd), (0.0, 0.0));
}

/// A failure status with no usage is unresolved: counted at the turn's bound, all of it
/// unresolved, and the run's `usd` covers it.
#[test]
fn a_failure_status_is_unresolved_at_its_bound() {
    let (dir, r) = scripted(
        "failure",
        serde_json::json!([{ "error": { "status": 400 } }]),
        None,
    );
    let bound = bound_of(&dir);
    assert!(bound > 0.0);
    assert!(
        close(r.spend.unresolved_usd, bound),
        "{:?} bound {bound}",
        r.spend
    );
    assert!(close(r.spend.usd, bound), "{:?}", r.spend);
}

/// A call cancelled while in flight is unresolved at its bound.
#[test]
fn a_call_cancelled_mid_flight_is_unresolved() {
    let (dir, r) = scripted(
        "mid-call",
        serde_json::json!([{ "stall_s": 30 }, submission()]),
        Some("turn_sent"),
    );
    assert_eq!(r.status, Status::Cancelled);
    let bound = bound_of(&dir);
    assert!(
        close(r.spend.unresolved_usd, bound),
        "{:?} bound {bound}",
        r.spend
    );
    assert!(close(r.spend.usd, bound), "{:?}", r.spend);
}

/// A cancel while waiting to retry charges the failed attempt once, not a second time for a
/// call that was never in flight.
#[test]
fn a_cancel_during_a_retry_wait_charges_only_the_failed_attempt() {
    let (dir, r) = scripted(
        "retry-wait",
        serde_json::json!([{ "error": { "status": 503, "retry_after_s": 30 } }, submission()]),
        Some("retry"),
    );
    assert_eq!(r.status, Status::Cancelled);
    let bound = bound_of(&dir);
    assert!(close(r.spend.usd, bound), "{:?} bound {bound}", r.spend);
    assert!(close(r.spend.unresolved_usd, bound), "{:?}", r.spend);
}

/// An amount the contract carries has at most six decimals (table-manners 4.6).
fn six_decimals(v: f64) -> bool {
    let scaled = v * 1e6;
    (scaled - scaled.round()).abs() < 1e-6
        && format!("{v}")
            .split('.')
            .nth(1)
            .is_none_or(|d| d.len() <= 6)
}

#[test]
fn spend_is_written_to_six_decimals() {
    let (_, r) = scripted(
        "decimals",
        serde_json::json!([{ "error": { "status": 400 } }]),
        None,
    );
    assert!(r.spend.usd > 0.0);
    assert!(six_decimals(r.spend.usd), "{}", r.spend.usd);
    assert!(
        six_decimals(r.spend.unresolved_usd),
        "{}",
        r.spend.unresolved_usd
    );
    let (_, held) = scripted("decimals-held", serde_json::json!([submission()]), None);
    assert!(six_decimals(held.spend.usd), "{}", held.spend.usd);
}

/// A retry the envelope cannot cover is refused, never sent: the failed attempt before it is
/// charged once, and the refusal adds no second, phantom charge (M8 review I2).
#[test]
fn a_refused_retry_adds_no_charge() {
    let invalid: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("fixtures/scripts/invalid-then-submit.json")).unwrap(),
    )
    .unwrap();
    // A first turn (an invalid submission, so a second turn follows) far over its bound eats
    // the envelope, so the next turn's retry is refused.
    let mut first = invalid[0].clone();
    first["usage"] = serde_json::json!({ "input_tokens": 50_000_000, "output_tokens": 0 });
    let (dir, r) = scripted(
        "refused-retry",
        serde_json::json!([first, { "error": { "status": 503 } }, submission()]),
        None,
    );
    assert_eq!(r.status, Status::Error, "{:?}", r.error);
    let journal = std::fs::read_to_string(dir.join("run-1/journal.jsonl")).unwrap();
    let charges: Vec<f64> = journal
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|e| e["event"] == "retry" || e["event"] == "attempt_failed")
        .filter_map(|e| e["data"]["charged_usd"].as_f64())
        .filter(|c| *c > 0.0)
        .collect();
    assert_eq!(
        charges.len(),
        1,
        "one charge, for the failed attempt: {charges:?}"
    );
    assert!(
        close(r.spend.unresolved_usd, charges[0]),
        "{:?} vs {charges:?}",
        r.spend
    );
}

/// A successful answer that reports no usage is not free: it counts the turn's bound, the
/// configured worst case, and the run's basis becomes `estimated` (table-manners 4.5). It is
/// settled by the answer, so none of it is unresolved.
#[test]
fn an_answer_without_usage_counts_its_bound_as_estimated() {
    let mut answer = submission();
    answer["usage"] = serde_json::Value::Null;
    let (dir, r) = scripted("no-usage", serde_json::json!([answer]), None);
    assert_eq!(r.status, Status::Completed);
    assert_eq!(r.spend.basis, table_manners::SpendBasis::Estimated);
    assert!(close(r.spend.usd, bound_of(&dir)), "{:?}", r.spend);
    assert_eq!(r.spend.unresolved_usd, 0.0);

    let (_, measured) = scripted("usage", serde_json::json!([submission()]), None);
    assert_eq!(measured.spend.basis, table_manners::SpendBasis::Measured);
}

/// A cancellation that lands before the decision's directory exists allocates nothing
/// (table-manners 4.2: checked again just before the run is allocated): no directory, no run.
#[test]
fn a_decision_cancelled_before_allocation_makes_no_run() {
    let out = tempdir("cancel-unallocated");
    let provider =
        ScriptedProvider::from_json(&serde_json::json!([submission()]).to_string()).unwrap();
    let tools = tools();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let cancel = run::Cancel::new();
    cancel.cancel();
    let services = run::Services::new(&provider, &tools).with_cancel(&cancel);
    let outcome = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &services,
        &out,
        "test",
    ));
    assert!(
        matches!(outcome, Err(squint_core::Error::Cancelled(_))),
        "{:?}",
        outcome.map(|o| o.dir)
    );
    assert_eq!(std::fs::read_dir(&out).map(|d| d.count()).unwrap_or(0), 0);
}

/// The association is in the run's directory from allocation, whole (written then renamed) and
/// naming the action, so a run still going is listed under its action (table-manners 4.3).
#[test]
fn the_association_names_the_action_and_is_written_whole() {
    let out = tempdir("association");
    let provider =
        ScriptedProvider::from_json(&serde_json::json!([submission()]).to_string()).unwrap();
    let tools = tools();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let association = squint_core::contract::Association {
        session_id: Some("session".into()),
        client_run_id: Some("client-1".into()),
        action: Some("source_fidelity".into()),
    };
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let told = seen.clone();
    let services = run::Services::new(&provider, &tools)
        .with_association(association.clone())
        .with_allocated(std::sync::Arc::new(move |dir: &Path| {
            *told.lock().unwrap() = Some(squint_core::contract::Association::read(dir));
        }));
    let outcome = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &services,
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(seen.lock().unwrap().as_ref(), Some(&association));
    assert_eq!(
        squint_core::contract::Association::read(&outcome.dir),
        association
    );
    let stray: Vec<_> = std::fs::read_dir(&outcome.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");
}

/// The directory's name and `run.json`'s `started` come from one clock reading, so the runs list
/// (ordered by the name) never disagrees with the times it shows.
#[test]
fn the_directory_name_carries_the_runs_start() {
    let (outcome, r) = run("named", "held.json", "pair.toml", &[], None);
    let name = outcome
        .dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let ms: u64 = name.split('-').nth(1).unwrap().parse().unwrap();
    assert_eq!(r.started, squint_core::contract::rfc3339(ms), "{name}");
}
