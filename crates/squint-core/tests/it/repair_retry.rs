//! Task 1.8: both repair forms (rejected response answered in history; non-replayable response
//! excluded and reissued), the repair budget, and transport retries through the run loop.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::provider::{ContentBlock, MessageRole};
use squint_core::run;
use squint_core::tool::{Envelope, Tool, ToolRegistry};
use squint_testkit::{ScriptedProvider, ScriptedTool};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-repair-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(overrides: &[&str]) -> Config {
    let text = std::fs::read_to_string(root().join("fixtures/config/fake-tool.toml")).unwrap();
    let ov: Vec<String> = overrides.iter().map(|s| s.to_string()).collect();
    Config::parse(&text, &ov, &|_| None).unwrap()
}

fn tools() -> ToolRegistry {
    let text = std::fs::read_to_string(root().join("fixtures/tool-scripts/regions-3-then-1.json"))
        .unwrap();
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

struct Run {
    outcome: run::Outcome,
    provider: ScriptedProvider,
    config: Config,
}

impl Run {
    fn report(&self) -> serde_json::Value {
        let text = std::fs::read_to_string(self.outcome.dir.join("report.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }
    fn run(&self) -> serde_json::Value {
        self.report()["runs"][0].clone()
    }
    fn events(&self) -> Vec<String> {
        std::fs::read_to_string(self.outcome.dir.join("run-1/journal.jsonl"))
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
    fn repairs(&self) -> Vec<(String, String)> {
        self.run()["repairs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["form"].as_str().unwrap().to_string(),
                    r["reason"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }
}

fn run_case(tag: &str, script: &str, overrides: &[&str]) -> Run {
    let out = tempdir(tag);
    let provider =
        ScriptedProvider::from_path(&root().join("fixtures/scripts").join(script)).unwrap();
    let config = config(overrides);
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let outcome = block_on(run::check(
        &config,
        &job,
        &root(),
        &run::Services::new(&provider, &tools()),
        &out,
        "test",
    ))
    .unwrap();
    Run {
        outcome,
        provider,
        config,
    }
}

fn text_of(block: &ContentBlock) -> &str {
    match block {
        ContentBlock::Text { text } => text,
        other => panic!("expected text, got {other:?}"),
    }
}

// ---- reissue form ----------------------------------------------------------------------------

#[test]
fn repair_reissues_a_truncated_turn_with_the_raised_allowance_and_excludes_it() {
    let r = run_case("trunc", "truncated-then-submit.json", &[]);
    assert_eq!(r.outcome.exit_code, 0, "{:?}", r.outcome.error);
    let recorded = r.provider.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(
        recorded[0].max_output_tokens,
        r.config.provider.max_output_tokens
    );
    assert_eq!(
        recorded[1].max_output_tokens,
        r.config.budget.repair_output_tokens
    );
    assert_eq!(
        recorded[0].history, recorded[1].history,
        "the excluded response never entered history"
    );
    let turns = r.run()["turns"].as_array().unwrap().clone();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[0]["entered_history"], false);
    assert_eq!(turns[1]["entered_history"], true);
    assert_eq!(
        r.repairs(),
        [("reissue".to_string(), "truncated".to_string())]
    );
    let ev = r.events();
    let excluded = ev.iter().position(|e| e == "turn_excluded").unwrap();
    assert_eq!(ev[excluded + 1], "repair");
    assert_eq!(ev.last().unwrap(), "verdict");
}

#[test]
fn repair_reissues_a_call_less_text_with_the_discarded_output_instruction() {
    let r = run_case("text", "text-then-submit.json", &[]);
    assert_eq!(r.outcome.exit_code, 0, "{:?}", r.outcome.error);
    let recorded = r.provider.recorded.lock().unwrap();
    assert_eq!(
        recorded[1].max_output_tokens, r.config.provider.max_output_tokens,
        "allowance unchanged"
    );
    assert_eq!(recorded[1].history.len(), recorded[0].history.len() + 1);
    let last = recorded[1].history.last().unwrap();
    assert_eq!(last.role, MessageRole::User);
    assert!(text_of(&last.content[0]).contains("previous output was discarded"));
    assert_eq!(
        r.repairs(),
        [("reissue".to_string(), "no_call".to_string())]
    );
}

#[test]
fn repair_reissues_a_refusal_the_same_way() {
    let r = run_case("refusal", "refusal-then-submit.json", &[]);
    assert_eq!(r.outcome.exit_code, 0, "{:?}", r.outcome.error);
    assert_eq!(
        r.repairs(),
        [("reissue".to_string(), "refusal".to_string())]
    );
    assert_eq!(r.run()["turns"][0]["entered_history"], false);
}

// ---- rejected form ---------------------------------------------------------------------------

#[test]
fn repair_answers_an_invalid_submission_in_history_with_the_validator_message() {
    let r = run_case("invalid", "invalid-then-submit.json", &[]);
    assert_eq!(r.outcome.exit_code, 0, "{:?}", r.outcome.error);
    let recorded = r.provider.recorded.lock().unwrap();
    assert_eq!(
        recorded[1].history.len(),
        recorded[0].history.len() + 2,
        "assistant call plus the repair message"
    );
    let repair = recorded[1].history.last().unwrap();
    match &repair.content[0] {
        ContentBlock::ToolResult {
            tool_use_id,
            content,
            is_error,
            ..
        } => {
            assert_eq!(tool_use_id, "call_1");
            assert!(*is_error);
            assert!(content.contains("chk:ob:SF2@candidate"), "{content}");
        }
        other => panic!("expected an error result, got {other:?}"),
    }
    assert!(text_of(&repair.content[1]).starts_with("Repair:"));
    assert_eq!(
        r.repairs(),
        [("rejected".to_string(), "invalid_submission".to_string())]
    );
    assert_eq!(r.run()["turns"][0]["entered_history"], true);
}

#[test]
fn repair_answers_a_mixed_batch_without_executing_any_call() {
    let r = run_case("mixed", "mixed-then-submit.json", &[]);
    assert_eq!(r.outcome.exit_code, 0, "{:?}", r.outcome.error);
    assert!(
        r.run()["tool_calls"].as_array().unwrap().is_empty(),
        "nothing executed"
    );
    let recorded = r.provider.recorded.lock().unwrap();
    let repair = recorded[1].history.last().unwrap();
    let errors = repair
        .content
        .iter()
        .filter(|b| matches!(b, ContentBlock::ToolResult { is_error: true, .. }))
        .count();
    assert_eq!(errors, 2, "one error result per call");
    assert_eq!(
        r.repairs(),
        [("rejected".to_string(), "mixed_batch".to_string())]
    );
}

#[test]
fn repair_after_a_stray_final_call_carries_the_final_instruction_again() {
    let r = run_case("stray", "stray-then-submit.json", &["budget.tool_turns=1"]);
    assert_eq!(r.outcome.exit_code, 0, "{:?}", r.outcome.error);
    assert_eq!(
        r.repairs(),
        [("rejected".to_string(), "stray_call".to_string())]
    );
    let recorded = r.provider.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    let repair = recorded[2].history.last().unwrap();
    assert!(matches!(
        &repair.content[0],
        ContentBlock::ToolResult { is_error: true, .. }
    ));
    assert!(text_of(&repair.content[1]).contains("Only submit_verdict will be executed now"));
}

// ---- budget ----------------------------------------------------------------------------------

#[test]
fn repair_budget_of_zero_makes_the_first_failure_terminal() {
    let r = run_case("zero", "truncated-then-submit.json", &["budget.repairs=0"]);
    assert_eq!(r.outcome.exit_code, 3);
    assert_eq!(r.run()["error"]["class"], "model_output");
    assert!(r.repairs().is_empty());
}

#[test]
fn repair_is_run_wide_so_two_different_failures_are_terminal() {
    // A truncated turn spends the repair; the invalid submission that follows has none left.
    let out = tempdir("two-kinds");
    let provider = ScriptedProvider::from_json(&format!(
        "[{},{}]",
        serde_json::to_string(
            &serde_json::from_str::<Vec<serde_json::Value>>(
                &std::fs::read_to_string(root().join("fixtures/scripts/truncated.json")).unwrap()
            )
            .unwrap()[0]
        )
        .unwrap(),
        serde_json::to_string(
            &serde_json::from_str::<Vec<serde_json::Value>>(
                &std::fs::read_to_string(root().join("fixtures/scripts/missing-item.json"))
                    .unwrap()
            )
            .unwrap()[0]
        )
        .unwrap(),
    ))
    .unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &run::Services::new(&provider, &tools()),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 3);
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["runs"][0]["error"]["class"], "submission_invalid");
    assert_eq!(report["runs"][0]["repairs"].as_array().unwrap().len(), 1);
}

// ---- retries ---------------------------------------------------------------------------------

#[test]
fn retry_of_a_429_is_journaled_and_counted_as_an_attempt() {
    let r = run_case("retry", "retry-then-submit.json", &[]);
    assert_eq!(r.outcome.exit_code, 0, "{:?}", r.outcome.error);
    let ev = r.events();
    let sent = ev.iter().position(|e| e == "turn_sent").unwrap();
    assert_eq!(&ev[sent..sent + 3], ["turn_sent", "retry", "turn_received"]);
    assert_eq!(r.run()["turns"][0]["attempts"], 2);
    assert_eq!(r.run()["turns"].as_array().unwrap().len(), 1);
    assert!(r.repairs().is_empty(), "a retry is not a repair");
}

#[test]
fn retry_is_refused_for_a_400_which_is_a_provider_error() {
    let r = run_case("bad-request", "bad-request.json", &[]);
    assert_eq!(r.outcome.exit_code, 3);
    // Not retryable either (task 9.6): the provider rejected the request as sent.
    assert_eq!(r.run()["error"]["class"], "provider_rejected");
    assert!(!r.events().iter().any(|e| e == "retry"));
    assert_eq!(
        r.provider.remaining(),
        1,
        "the submission was never requested"
    );
}

#[test]
fn retry_budget_of_zero_fails_on_the_first_transport_error() {
    let r = run_case(
        "no-retries",
        "retry-then-submit.json",
        &["budget.retries=0"],
    );
    assert_eq!(r.outcome.exit_code, 3);
    assert_eq!(r.run()["error"]["class"], "provider");
}

// ---- added at the M1 checkpoint from the coverage review ------------------------------------

#[test]
fn repair_on_the_final_turn_reissues_a_truncated_answer_with_the_raised_allowance() {
    let r = run_case(
        "final-trunc",
        "final-truncated-then-submit.json",
        &["budget.tool_turns=1"],
    );
    assert_eq!(r.outcome.exit_code, 0, "{:?}", r.outcome.error);
    let recorded = r.provider.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    assert_eq!(
        recorded[2].max_output_tokens,
        r.config.budget.repair_output_tokens
    );
    assert_eq!(
        recorded[1].history, recorded[2].history,
        "the excluded answer never entered history"
    );
    assert!(matches!(
        &recorded[2].history.last().unwrap().content[0],
        ContentBlock::Text { text } if text.contains("Only submit_verdict will be executed now")
    ));
    assert_eq!(
        r.repairs(),
        [("reissue".to_string(), "truncated".to_string())]
    );
}

#[test]
fn repair_on_the_final_turn_answers_an_invalid_submission_with_the_final_instruction_again() {
    let r = run_case(
        "final-invalid",
        "final-invalid-then-submit.json",
        &["budget.tool_turns=1"],
    );
    assert_eq!(r.outcome.exit_code, 0, "{:?}", r.outcome.error);
    assert_eq!(
        r.repairs(),
        [("rejected".to_string(), "invalid_submission".to_string())]
    );
    let recorded = r.provider.recorded.lock().unwrap();
    let repair = recorded[2].history.last().unwrap();
    assert!(matches!(
        &repair.content[0],
        ContentBlock::ToolResult { is_error: true, .. }
    ));
    assert!(text_of(&repair.content[1]).contains("Only submit_verdict will be executed now"));
}

#[test]
fn repair_does_not_consume_a_tool_turn() {
    // One tool turn allowed: a mixed batch is repaired, then a real batch still fits in the turn.
    let r = run_case(
        "no-turn",
        "mixed-then-batch-then-submit.json",
        &["budget.tool_turns=1"],
    );
    assert_eq!(r.outcome.exit_code, 0, "{:?}", r.outcome.error);
    let run = r.run();
    assert_eq!(
        run["tool_calls"].as_array().unwrap().len(),
        1,
        "the batch after the repair executed"
    );
    assert_eq!(run["tool_calls"][0]["status"], "ok");
    assert_eq!(
        r.repairs(),
        [("rejected".to_string(), "mixed_batch".to_string())]
    );
    assert!(!r.events().iter().any(|e| e == "turn_excluded"));
}
