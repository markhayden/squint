//! Task 1.9: the decision envelope is planned from the resolved run, reserved before any request,
//! shown in the report, and a cap below it ends the run `error: budget` with nothing sent.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::ledger::{CapLedger, Ledger};
use squint_core::run;
use squint_core::tool::ToolRegistry;
use squint_testkit::ScriptedProvider;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-ledger-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(overrides: &[&str]) -> Config {
    let text = std::fs::read_to_string(root().join("fixtures/config/minimal.toml")).unwrap();
    let ov: Vec<String> = overrides.iter().map(|s| s.to_string()).collect();
    Config::parse(&text, &ov, &|_| None).unwrap()
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn check(
    tag: &str,
    script: &str,
    overrides: &[&str],
    shared: Option<&dyn Ledger>,
) -> (run::Outcome, serde_json::Value) {
    let out = tempdir(tag);
    let provider =
        ScriptedProvider::from_path(&root().join("fixtures/scripts").join(script)).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(overrides),
        &job,
        &root(),
        &run::Services::new(&provider, &tools).with_shared_ledger(shared),
        &out,
        "test",
    ))
    .unwrap();
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    (o, report)
}

fn events(o: &run::Outcome) -> Vec<String> {
    std::fs::read_to_string(o.dir.join("run-1/journal.jsonl"))
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect()
}

#[test]
fn ledger_envelope_is_reserved_and_the_report_shows_it() {
    let (o, r) = check("fits", "held.json", &[], None);
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let cost = &r["cost"];
    let envelope = cost["envelope"]["usd"].as_f64().unwrap();
    assert!(envelope > 0.01 && envelope < 2.0, "envelope {envelope}");
    assert_eq!(
        cost["envelope"]["requests"].as_array().unwrap().len(),
        6,
        "4 tool turns, final, repair"
    );
    assert_eq!(cost["attempts"], 1);
    assert_eq!(cost["spent_usd"], 0.0, "scripted usage is zero");
    assert!(
        (cost["released_usd"].as_f64().unwrap() - envelope).abs() < 1e-9,
        "everything comes back"
    );
    let turn = &r["runs"][0]["turns"][0];
    assert!(turn["bound_usd"].as_f64().unwrap() > 0.0);
    assert_eq!(turn["cost_usd"], 0.0);
    let ev = events(&o);
    assert!(
        ev.iter().position(|e| e == "reserved").unwrap()
            < ev.iter().position(|e| e == "turn_sent").unwrap()
    );
}

/// The envelope plans as many repairs as the budget allows, so `repairs = 2` is not under-planned.
#[test]
fn ledger_envelope_plans_every_allowed_repair() {
    let (_, r) = check("two-repairs", "held.json", &["budget.repairs=2"], None);
    let requests = r["cost"]["envelope"]["requests"].as_array().unwrap();
    assert_eq!(requests.len(), 7, "4 tool turns, final, two repairs");
    let kinds: Vec<&str> = requests
        .iter()
        .map(|q| q["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k == "repair").count(), 2);
    assert_eq!(kinds[4], "final_turn");
}

#[test]
fn ledger_refuses_a_cap_below_the_envelope_before_any_request() {
    let (o, r) = check("cap", "held.json", &["budget.spend_cap_usd=0.01"], None);
    assert_eq!(o.exit_code, 3);
    assert_eq!(r["runs"][0]["error"]["class"], "budget");
    assert!(
        r["runs"][0]["turns"].as_array().unwrap().is_empty(),
        "nothing was sent"
    );
    assert_eq!(events(&o), ["resolved", "prepared", "error"]);
    assert!(
        r["cost"]["envelope"]["usd"].as_f64().unwrap() > 0.01,
        "the report still shows the envelope"
    );
}

#[test]
fn ledger_shared_across_decisions_refuses_the_second_when_it_does_not_fit() {
    let shared = CapLedger::new(0.5);
    let (first, r) = check("shared-1", "held.json", &[], Some(&shared));
    assert_eq!(first.exit_code, 0, "{:?}", first.error);
    let envelope = r["cost"]["envelope"]["usd"].as_f64().unwrap();
    assert!(
        envelope > 0.05 && envelope < 0.5,
        "one decision ({envelope}) fits the 0.5 cap with room to spare"
    );
    assert!(
        shared.reserved().abs() < 1e-9 && shared.spent().abs() < 1e-9,
        "settled and released"
    );
    // Lower the shared cap's headroom by spending it.
    shared.reserve(0.5 - envelope / 2.0).unwrap();
    let (second, r2) = check("shared-2", "held.json", &[], Some(&shared));
    assert_eq!(second.exit_code, 3);
    assert_eq!(r2["runs"][0]["error"]["class"], "budget");
}

#[test]
fn ledger_counts_a_retry_as_a_charged_attempt() {
    let (o, r) = check("retry", "retry-then-submit.json", &[], None);
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let turn = &r["runs"][0]["turns"][0];
    assert_eq!(turn["attempts"], 2);
    let bound = turn["bound_usd"].as_f64().unwrap();
    assert!(
        (turn["cost_usd"].as_f64().unwrap() - bound).abs() < 1e-12,
        "the failed attempt is charged at its bound"
    );
    assert!((r["cost"]["spent_usd"].as_f64().unwrap() - bound).abs() < 1e-12);
    assert_eq!(r["cost"]["attempts"], 2);
}

// ---- added at the M1 checkpoint from the code review ----------------------------------------

#[test]
fn ledger_charges_the_final_failed_attempt_at_its_bound() {
    let (o, r) = check("failed-attempt", "bad-request.json", &[], None);
    assert_eq!(o.exit_code, 3);
    assert_eq!(r["runs"][0]["error"]["class"], "provider_rejected");
    let turn = &r["runs"][0]["turns"][0];
    let bound = turn["bound_usd"].as_f64().unwrap();
    assert!(bound > 0.0);
    assert!(
        (turn["cost_usd"].as_f64().unwrap() - bound).abs() < 1e-12,
        "the 400 attempt is charged"
    );
    assert_eq!(turn["attempts"], 1);
    assert!((r["cost"]["spent_usd"].as_f64().unwrap() - bound).abs() < 1e-12);
    assert!(events(&o).iter().any(|e| e == "attempt_failed"));
}

#[test]
fn ledger_shared_reservation_is_released_even_when_the_journal_fails() {
    let shared = CapLedger::new(10.0);
    let out = tempdir("journal-fault");
    let provider = ScriptedProvider::from_path(&root().join("fixtures/scripts/held.json")).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let mut services = run::Services::new(&provider, &tools).with_shared_ledger(Some(&shared));
    services.fail_journal_at = Some(3);
    let err = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &services,
        &out,
        "test",
    ))
    .unwrap_err();
    assert!(err.to_string().contains("injected journal fault"), "{err}");
    assert!(
        shared.reserved().abs() < 1e-9,
        "nothing left reserved: {}",
        shared.reserved()
    );
}
