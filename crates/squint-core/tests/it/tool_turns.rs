//! Task 1.7a: tool turns through the run state machine with a scripted provider and a scripted
//! tool: first look then submit, two tool turns, a batch over the call budget, invalid arguments,
//! a tool crash, and the provider-history invariant.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::config::Config;
use squint_core::image::{ImagePlan, ManyImages, RequestImageLimits};
use squint_core::job::Job;
use squint_core::provider::{ContentBlock, MessageRole, Provider, TurnResponse};
use squint_core::run;
use squint_core::tool::{Envelope, Tool, ToolRegistry};
use squint_testkit::{ScriptedProvider, ScriptedTool};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-tool-turns-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(overrides: &[&str]) -> Config {
    let text = std::fs::read_to_string(root().join("fixtures/config/fake-tool.toml")).unwrap();
    let ov: Vec<String> = overrides.iter().map(|s| s.to_string()).collect();
    Config::parse(&text, &ov, &|_| None).unwrap()
}

fn provider(script: &str) -> ScriptedProvider {
    let text = std::fs::read_to_string(root().join("fixtures/scripts").join(script)).unwrap();
    let turns: Vec<TurnResponse> = serde_json::from_str(&text).unwrap();
    ScriptedProvider::new(turns)
}

fn tools(tool_script: &str) -> ToolRegistry {
    let text =
        std::fs::read_to_string(root().join("fixtures/tool-scripts").join(tool_script)).unwrap();
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

fn report(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap()
}

fn journal_events(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("run-1/journal.jsonl"))
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
fn two_tool_turns_then_submit() {
    let out = tempdir("two-turns");
    let p = provider("two-tool-turns.json");
    let t = tools("regions-3-then-1.json");
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &run::Services::new(&p, &t),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let r = report(&o.dir);
    let run = &r["runs"][0];
    assert_eq!(run["turns"].as_array().unwrap().len(), 3);
    assert_eq!(run["tool_calls"].as_array().unwrap().len(), 2);
    assert_eq!(
        run["tool_calls"][0]["added_items"],
        serde_json::json!(["chk:ev:1:0", "chk:ev:1:1"])
    );
    assert_eq!(run["tool_calls"][0]["dropped"], serde_json::json!(["r2"]));
    assert_eq!(
        run["tool_calls"][1]["added_items"],
        serde_json::json!(["chk:ev:2:0"])
    );
    let ids: Vec<&str> = run["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "chk:ob:SF1@candidate",
            "chk:ob:SF2@candidate",
            "chk:ev:1:0",
            "chk:ev:1:1",
            "chk:ev:2:0"
        ]
    );
    let events = journal_events(&o.dir);
    assert_eq!(events.iter().filter(|e| *e == "tool_called").count(), 2);
    assert_eq!(
        events.iter().filter(|e| *e == "evidence_dropped").count(),
        1
    );
    assert!(events.contains(&"prepared".to_string()) && events.contains(&"reserved".to_string()));
    assert_eq!(r["tools"], serde_json::json!(["fake_regions@0.0.1"]));
    assert_eq!(r["prompt_version"], "v1");

    // Provider history is append-only: every request extends the previous one; the system prompt
    // and the tool definitions never change.
    let recorded = p.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 3);
    for pair in recorded.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        assert_eq!(a.system, b.system);
        assert_eq!(a.tools, b.tools, "tool definitions are frozen");
        assert!(b.history.len() > a.history.len());
        assert_eq!(
            &b.history[..a.history.len()],
            &a.history[..],
            "earlier messages are unchanged"
        );
    }
    assert_eq!(
        recorded[0]
            .tools
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        ["fake_regions", "submit_verdict"]
    );
    assert!(
        recorded[0].system.contains("Tools."),
        "the Tools section is rendered"
    );
    // The second request carries the first call's result and the updated checklist.
    let last = recorded[1].history.last().unwrap();
    assert_eq!(last.role, MessageRole::User);
    assert!(matches!(
        &last.content[0],
        ContentBlock::ToolResult {
            is_error: false,
            ..
        }
    ));
    assert!(matches!(&last.content[1], ContentBlock::Text { text } if text.contains("chk:ev:1:0")));
}

#[test]
fn first_look_then_submit_with_no_tools_enabled() {
    let out = tempdir("no-tools");
    let p = provider("held.json");
    let t = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&["tools.enabled=[]"]),
        &job,
        &root(),
        &run::Services::new(&p, &t),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let recorded = p.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].tools.len(), 1, "only submit_verdict");
    assert!(
        !recorded[0].system.contains("Tools."),
        "no Tools section without tools"
    );
}

#[test]
fn a_batch_over_the_call_budget_is_answered_with_errors_then_the_final_turn() {
    let out = tempdir("over-budget");
    let p = provider("batch-over-budget.json");
    let t = tools("regions-3-then-1.json");
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&["budget.tool_calls=1"]),
        &job,
        &root(),
        &run::Services::new(&p, &t),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let recorded = p.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    let after_batch = &recorded[1].history;
    // assistant (2 calls), user (2 error results), user (final instruction)
    let results = &after_batch[after_batch.len() - 2];
    assert_eq!(
        results
            .content
            .iter()
            .filter(|c| matches!(c, ContentBlock::ToolResult { is_error: true, .. }))
            .count(),
        2
    );
    let instruction = &after_batch[after_batch.len() - 1];
    assert!(
        matches!(&instruction.content[0], ContentBlock::Text { text } if text.contains("Only submit_verdict"))
    );
    let r = report(&o.dir);
    let calls = r["runs"][0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2, "both refused calls are on record");
    assert!(
        calls
            .iter()
            .all(|c| c["executed"] == false && c["reason"] == "call budget"),
        "nothing was executed: {calls:?}"
    );
}

#[test]
fn invalid_arguments_are_error_results_not_failures() {
    let out = tempdir("invalid");
    let p = provider("invalid-args.json");
    let t = tools("regions-3-then-1.json");
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &run::Services::new(&p, &t),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let r = report(&o.dir);
    let calls = r["runs"][0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2);
    assert!(
        calls[0]["invalid_args"]
            .as_str()
            .unwrap()
            .contains("_squint")
    );
    assert!(
        calls[1]["invalid_args"]
            .as_str()
            .unwrap()
            .contains("unknown image id")
    );
    let recorded = p.recorded.lock().unwrap();
    let after_first = &recorded[1].history;
    assert!(
        matches!(&after_first.last().unwrap().content[0], ContentBlock::ToolResult { is_error: true, content, .. } if content.contains("_squint"))
    );
}

#[test]
fn an_image_argument_given_as_its_raw_frame_resolves_to_the_image() {
    // The prompt introduces each image with its raw frame (`raw:candidate`), and models pass that
    // where a tool wants the image id. The harness accepts it, hands the tool the image id, and
    // journals what the model said.
    let out = tempdir("raw-frame-arg");
    let p = provider("raw-frame-image-arg.json");
    let t = tools("measured-nothing.json");
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &run::Services::new(&p, &t),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let r = report(&o.dir);
    let calls = r["runs"][0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["executed"], true, "{:?}", calls[0]);
    assert!(calls[0]["invalid_args"].is_null(), "{:?}", calls[0]);
    assert_eq!(calls[0]["arguments"]["image"], "raw:candidate");
    assert!(journal_events(&o.dir).contains(&"tool_returned".to_string()));
}

#[test]
fn a_raw_frame_of_an_unknown_image_is_still_refused_in_the_models_words() {
    // Only `raw:<image>` of a known image resolves; anything else is refused quoting what the
    // model sent, and a frame argument is never rewritten.
    let out = tempdir("raw-frame-unknown");
    let p = provider("raw-frame-unknown.json");
    let t = tools("measured-nothing.json");
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &run::Services::new(&p, &t),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let r = report(&o.dir);
    let calls = r["runs"][0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["executed"], false);
    let refusal = calls[0]["invalid_args"].as_str().unwrap();
    assert!(
        refusal.contains("unknown image id \"raw:nope\""),
        "{refusal}"
    );
    assert!(!journal_events(&o.dir).contains(&"tool_returned".to_string()));
}

#[test]
fn a_tool_crash_is_an_operational_error() {
    let out = tempdir("crash");
    let p = provider("tool-crash.json");
    let t = tools("empty.json");
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &run::Services::new(&p, &t),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 3);
    let r = report(&o.dir);
    assert_eq!(r["runs"][0]["error"]["class"], "tool");
    assert!(r["verdict"].is_null());
}

// ---- added at the M1 checkpoint from the coverage review ------------------------------------

fn check_with(
    tag: &str,
    script: &str,
    tool_script: &str,
    overrides: &[&str],
) -> (run::Outcome, ScriptedProvider) {
    let out = tempdir(tag);
    let p = provider(script);
    let t = tools(tool_script);
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(overrides),
        &job,
        &root(),
        &run::Services::new(&p, &t),
        &out,
        "test",
    ))
    .unwrap();
    (o, p)
}

#[test]
fn a_call_budget_exactly_exhausted_goes_to_the_final_turn() {
    let (o, p) = check_with(
        "exact-budget",
        "one-call.json",
        "regions-3-then-1.json",
        &["budget.tool_calls=1"],
    );
    // The scripted submission omits the evidence items, so a repair and then the exhausted script
    // follow; what matters here is that request 2 carried the final instruction.
    assert_eq!(o.exit_code, 3);
    let recorded = p.recorded.lock().unwrap();
    assert!(recorded.len() >= 2);
    assert!(matches!(
        &recorded[1].history.last().unwrap().content[0],
        ContentBlock::Text { text } if text.starts_with("Tool budget exhausted.")
    ));
    assert_eq!(
        report(&o.dir)["runs"][0]["tool_calls"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn an_envelope_over_its_bounds_is_an_operational_error() {
    let (o, p) = check_with("over-bound", "one-call.json", "over-bound.json", &[]);
    assert_eq!(o.exit_code, 3);
    let r = report(&o.dir);
    assert_eq!(r["runs"][0]["error"]["class"], "tool");
    assert!(
        r["runs"][0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("regions")
    );
    let ev = journal_events(&o.dir);
    assert!(ev.contains(&"tool_called".to_string()) && !ev.contains(&"tool_returned".to_string()));
    assert_eq!(p.remaining(), 1, "the submission was never requested");
}

#[test]
fn a_returned_image_with_an_unknown_parent_is_an_operational_error() {
    let (o, _) = check_with("bad-parent", "one-call.json", "bad-parent.json", &[]);
    assert_eq!(o.exit_code, 3);
    let r = report(&o.dir);
    assert_eq!(r["runs"][0]["error"]["class"], "tool");
    assert!(
        r["runs"][0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not a frame")
    );
}

#[test]
fn measured_nothing_and_not_applicable_results_are_evidence_a_submission_may_cite() {
    for script in ["measured-nothing.json", "not-applicable.json"] {
        let (o, _) = check_with(script, "one-call-cite.json", script, &[]);
        assert_eq!(o.exit_code, 0, "{script}: {:?}", o.error);
        let r = report(&o.dir);
        assert_eq!(
            r["runs"][0]["tool_calls"][0]["status"],
            script.trim_end_matches(".json").replace('-', "_")
        );
        assert!(
            r["runs"][0]["checklist"]["items"].as_array().unwrap().len() == 2,
            "no evidence items added"
        );
    }
}

// ---- added at the M1 checkpoint from the code review ----------------------------------------

#[test]
fn a_returned_image_becomes_a_crop_frame_the_report_records() {
    let (o, _) = check_with("crop-image", "one-call-cite.json", "crop-image.json", &[]);
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let r = report(&o.dir);
    let sent = r["runs"][0]["images_sent"].as_object().unwrap();
    assert!(
        sent.contains_key("crop:call:1:0"),
        "{:?}",
        sent.keys().collect::<Vec<_>>()
    );
    assert!(o.dir.join("run-1/frames/crop_call_1_0.png").is_file());
    // The frame tree is in the report, so a viewer can map the crop's locations to raw
    // coordinates of the candidate.
    let frames = &r["runs"][0]["frames"];
    assert_eq!(frames["raw:candidate"]["root"], "candidate");
    assert!(frames["raw:candidate"]["width"].as_u64().unwrap() > 0);
    assert!(frames["raw:candidate"].get("parent").is_none());
    let crop = &frames["crop:call:1:0"];
    assert_eq!(crop["root"], "candidate");
    assert_eq!(crop["parent"], "raw:candidate");
    assert_eq!(
        crop["mapping"]["bbox"],
        serde_json::json!([0.0, 0.0, 0.5, 0.5])
    );
    assert_eq!(
        (crop["width"].as_u64(), crop["height"].as_u64()),
        (Some(32), Some(24))
    );
}

#[test]
fn a_returned_image_may_not_rename_itself_or_replace_a_frame_or_come_without_bytes() {
    for (script, message) in [
        ("overwrite-frame.json", "must be"),
        ("image-without-bytes.json", "carries no bytes"),
    ] {
        let (o, p) = check_with(script, "one-call-cite.json", script, &[]);
        assert_eq!(o.exit_code, 3, "{script}");
        let r = report(&o.dir);
        assert_eq!(r["runs"][0]["error"]["class"], "tool", "{script}");
        assert!(
            r["runs"][0]["error"]["message"]
                .as_str()
                .unwrap()
                .contains(message),
            "{script}: {}",
            r["runs"][0]["error"]["message"]
        );
        assert_eq!(
            p.remaining(),
            1,
            "{script}: the submission was never requested"
        );
        assert!(
            !o.dir.join("run-1/frames/crop_call_1_0.png").exists(),
            "{script}"
        );
    }
}

#[test]
fn a_cancel_during_a_slow_tool_call_lands_without_waiting_for_the_tool() {
    let out = tempdir("cancel-tool");
    let p = provider("one-call.json");
    let text = std::fs::read_to_string(root().join("fixtures/tool-scripts/regions-3-then-1.json"))
        .unwrap();
    let envelopes: Vec<Envelope> = serde_json::from_str(&text).unwrap();
    let slow: Vec<Box<dyn Tool>> = vec![Box::new(
        ScriptedTool::fake_regions(envelopes).with_delay(std::time::Duration::from_secs(3)),
    )];
    let t = ToolRegistry::new(slow, &["fake_regions".into()], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let cancel = run::Cancel::new();
    let config = config(&[]);
    let base = root();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    let o = runtime.block_on(async {
        let services = run::Services::new(&p, &t).with_cancel(&cancel);
        let run = run::check(&config, &job, &base, &services, &out, "test");
        let trigger = async {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            cancel.cancel();
        };
        let (o, ()) = tokio::join!(run, trigger);
        o.unwrap()
    });
    assert!(
        started.elapsed() < std::time::Duration::from_millis(2500),
        "the tool's 3 s did not block the cancel: {:?}",
        started.elapsed()
    );
    assert_eq!(o.exit_code, 3);
    let r = report(&o.dir);
    assert_eq!(r["runs"][0]["error"]["class"], "cancelled");
    assert_eq!(journal_events(&o.dir).last().unwrap(), "cancelled");
}

#[test]
fn refused_calls_appear_in_the_report_as_not_executed() {
    let (o, _) = check_with(
        "refused",
        "stray-then-submit.json",
        "regions-3-then-1.json",
        &["budget.tool_turns=1"],
    );
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let r = report(&o.dir);
    let calls = r["runs"][0]["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(calls[0]["executed"], false);
    assert!(
        calls[0]["invalid_args"]
            .as_str()
            .unwrap()
            .contains("unknown image id")
    );
    assert_eq!(calls[1]["executed"], false);
    assert_eq!(calls[1]["reason"], "final_turn");
    assert_eq!(calls[1]["id"], calls[1]["provider_call_id"]);
}

// ---- task 4.4b: the paths a tool is handed are absolute ---------------------------------------

#[test]
fn a_tool_is_handed_absolute_paths_to_its_pixels() {
    let out = tempdir("absolute");
    let relative = {
        // A relative output directory, as the default config has.
        let cwd = std::env::current_dir().unwrap();
        pathdiff(&out, &cwd)
    };
    let p = provider("one-call.json");
    let text = std::fs::read_to_string(root().join("fixtures/tool-scripts/regions-3-then-1.json"))
        .unwrap();
    let envelopes: Vec<Envelope> = serde_json::from_str(&text).unwrap();
    let scripted = std::sync::Arc::new(ScriptedTool::fake_regions(envelopes));
    let registry = ToolRegistry::new(
        vec![Box::new(SharedTool(std::sync::Arc::clone(&scripted)))],
        &["fake_regions".into()],
        &[],
    )
    .unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &run::Services::new(&p, &registry),
        &relative,
        "test",
    ))
    .unwrap();
    let calls = scripted.calls.lock().unwrap();
    let ctx = &calls[0].1;
    let path = std::path::Path::new(ctx.images["candidate"].path.as_deref().unwrap());
    assert!(path.is_absolute(), "{path:?}");
    assert!(path.is_file());
    assert!(ctx.run_dir.as_ref().unwrap().is_absolute());
}

/// The scripted tool shared with the test so its recorded calls can be read afterwards.
struct SharedTool(std::sync::Arc<ScriptedTool>);

impl Tool for SharedTool {
    fn manifest(&self) -> &squint_core::tool::Manifest {
        self.0.manifest()
    }
    fn call(
        &self,
        args: &serde_json::Value,
        ctx: &squint_core::tool::SquintContext,
    ) -> squint_core::Result<Envelope> {
        self.0.call(args, ctx)
    }
}

/// `target` relative to `base` when `target` is under it, else `target` itself.
fn pathdiff(target: &std::path::Path, base: &std::path::Path) -> PathBuf {
    target
        .strip_prefix(base)
        .map(PathBuf::from)
        .unwrap_or_else(|_| target.to_path_buf())
}

// ---- task 4.1: evaluation projections ---------------------------------------------------------

/// A tool under `visual_only` (the control arms' `align`) returns its pictures and nothing the
/// checklist could take: no regions, no measurements, a fixed summary. The report says which
/// projection the call ran under, and the stored envelope is the projected one.
#[test]
fn a_projected_tool_adds_no_evidence_and_the_report_names_the_projection() {
    let out = tempdir("projected");
    let p = provider("one-call.json");
    let text = std::fs::read_to_string(root().join("fixtures/tool-scripts/regions-3-then-1.json"))
        .unwrap();
    let envelopes: Vec<Envelope> = serde_json::from_str(&text).unwrap();
    let t: Vec<Box<dyn Tool>> = vec![Box::new(ScriptedTool::fake_regions(envelopes))];
    let projections = [("fake_regions".to_string(), "visual_only".to_string())]
        .into_iter()
        .collect();
    let registry = ToolRegistry::new(t, &["fake_regions".into()], &[])
        .unwrap()
        .with_projections(&projections)
        .unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &run::Services::new(&p, &registry),
        &out,
        "test",
    ))
    .unwrap();
    let r = report(&o.dir);
    let call = &r["runs"][0]["tool_calls"][0];
    assert_eq!(call["executed"], true);
    assert_eq!(call["projection"], "visual_only");
    assert_eq!(call["added_items"].as_array().unwrap().len(), 0, "{call}");
    assert_eq!(
        r["runs"][0]["checklist"]["items"].as_array().unwrap().len(),
        2,
        "the static items only"
    );
    let sha = call["envelope"]["sha256"].as_str().unwrap();
    let stored: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(o.dir.join("run-1/artifacts").join(sha)).unwrap(),
    )
    .unwrap();
    assert_eq!(stored["regions"].as_array().unwrap().len(), 0);
    assert_eq!(stored["summary"], "Result not available.");
    assert_eq!(stored["status"], "not_applicable");
    // The model saw the projected result too.
    let recorded = p.recorded.lock().unwrap();
    let shown = serde_json::to_string(&recorded[1].history).unwrap();
    assert!(shown.contains("Result not available."), "{shown}");
    assert!(
        !shown.contains("chk:ev:"),
        "no evidence item reached the model: {shown}"
    );
}

// ---- F1: images that can pass the many-image threshold, decided at resolve ------------------

/// The scripted model's limits: `max` images per request, and at most 32 pixels per side once a
/// request carries more than `threshold`. The pair's images are 64x48 and the crop 32x24.
fn limits(max: u32, threshold: u32) -> RequestImageLimits {
    RequestImageLimits {
        max_per_request: max,
        many: Some(ManyImages {
            threshold,
            max_long_edge: 32,
        }),
    }
}

/// One tool call returning a crop, then the submission, against a model with `limits`: the
/// default budget of 8 calls of one image each puts the worst case at 2 + 8 = 10 images.
fn check_limited(
    tag: &str,
    limits: Option<RequestImageLimits>,
) -> (run::Outcome, ScriptedProvider) {
    let out = tempdir(tag);
    let p = provider("one-call-cite.json");
    let p = match limits {
        Some(l) => p.with_image_limits(l),
        None => p,
    };
    let t = tools("crop-image.json");
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &run::Services::new(&p, &t),
        &out,
        "test",
    ))
    .unwrap();
    (o, p)
}

fn run_json(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("run.json")).unwrap()).unwrap()
}

#[test]
fn a_run_that_can_pass_the_many_image_threshold_is_prepared_smaller_from_the_first_request() {
    // Threshold 2: the second request carries three images. Prepared at the usual size, the 64x48
    // job images fail that request mid-run, after the first was paid for.
    let (o, p) = check_limited("many-images", Some(limits(20, 2)));
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let recorded = p.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(
        recorded[0].image_sizes["candidate"],
        (32, 24),
        "smaller from the first request"
    );
    assert_eq!(recorded[0].image_sizes["source"], (32, 24));
    assert_eq!(
        recorded[1].image_sizes.len(),
        3,
        "the crop crossed the threshold"
    );
    let r = report(&o.dir);
    assert_eq!(r["runs"][0]["prep_version"], "canonical+many32");
    // The spend stays within the reservation the estimate made.
    assert_eq!(r["cost"]["overrun_usd"], 0.0);
    assert!(
        r["cost"]["spent_usd"].as_f64().unwrap() <= r["cost"]["envelope"]["usd"].as_f64().unwrap()
    );
}

#[test]
fn a_run_whose_worst_case_cannot_fit_the_per_request_count_is_refused_before_any_spend() {
    let (o, p) = check_limited("too-many-images", Some(limits(5, 2)));
    assert_ne!(o.exit_code, 0);
    assert!(p.recorded.lock().unwrap().is_empty(), "no request was sent");
    assert_eq!(p.remaining(), 2);
    let r = report(&o.dir);
    let error = &r["runs"][0]["error"];
    assert_eq!(error["class"], "image_limit", "{error}");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("can carry 10 images (2 from the job and up to 1 from each of 8 tool calls), over the model's limit of 5 per request"),
        "{error}"
    );
    assert_eq!(r["cost"]["spent_usd"], 0.0);
    assert_eq!(r["cost"]["attempts"], 0);
    let events = journal_events(&o.dir);
    assert!(
        !events.iter().any(|e| e == "reserved" || e == "turn_sent"),
        "{events:?}"
    );
    assert_eq!(run_json(&o.dir)["error"]["code"], "input");
}

#[test]
fn a_run_that_cannot_pass_the_threshold_prepares_exactly_as_before() {
    // A worst case of 10 against a threshold of 10: the usual preparation, the same requests as a
    // model with no limits at all, and the same input identity.
    let (o, p) = check_limited("at-threshold", Some(limits(20, 10)));
    let (plain, q) = check_limited("no-limits", None);
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    assert_eq!(plain.exit_code, 0, "{:?}", plain.error);
    let (a, b) = (p.recorded.lock().unwrap(), q.recorded.lock().unwrap());
    assert_eq!(a[0].image_sizes["candidate"], (64, 48));
    assert_eq!(*a, *b);
    assert_eq!(report(&o.dir)["runs"][0]["prep_version"], "canonical");
    assert_eq!(
        run_json(&o.dir)["input_identity"],
        run_json(&plain.dir)["input_identity"]
    );
}

#[test]
fn the_prep_version_is_part_of_the_input_identity_and_the_cache_key() {
    // The same config, job and tools: only the model's limits differ, so only the preparation.
    let (many, _) = check_limited("identity-many", Some(limits(20, 2)));
    let (usual, _) = check_limited("identity-usual", None);
    assert_ne!(
        run_json(&many.dir)["input_identity"],
        run_json(&usual.dir)["input_identity"]
    );
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let t = tools("crop-image.json");
    let key = |p: &ScriptedProvider| {
        let setup = run::setup(&config(&[]), &job, &root(), &t, p.pricing()).unwrap();
        let plan = setup.image_plan().unwrap();
        squint_core::cache::KeyInput::gather(&config(&[]), &job, &setup, plan, &t, p, "test")
            .key()
            .unwrap()
    };
    let plain = ScriptedProvider::new(vec![]);
    let limited = ScriptedProvider::new(vec![]).with_image_limits(limits(20, 2));
    assert_ne!(key(&plain), key(&limited));
}

#[test]
fn the_estimate_reflects_the_image_plan() {
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let t = tools("crop-image.json");
    let pricing = |l: Option<RequestImageLimits>| squint_testkit::ScriptedPricing {
        prices: squint_testkit::FAKE_PRICES,
        image_limits: l,
    };
    // Prepared smaller: planned over the worst case of ten images, the count the plan decided on.
    let many = pricing(Some(limits(20, 2)));
    let setup = run::setup(&config(&[]), &job, &root(), &t, Some(&many)).unwrap();
    assert_eq!(
        setup.image_plan().unwrap(),
        ImagePlan::Many { long_edge: 32 }
    );
    let envelope = run::envelope(&config(&[]), &job, &root(), &t, &many).unwrap();
    assert_eq!(envelope.requests.last().unwrap().shape.images, 10);
    // Refused: the estimate is refused with the run's own error, before any envelope.
    let err = run::envelope(
        &config(&[]),
        &job,
        &root(),
        &t,
        &pricing(Some(limits(5, 2))),
    )
    .unwrap_err();
    assert_eq!(err.class(), "image_limit");
    let config = config(&[]);
    let refusing = pricing(Some(limits(5, 2)));
    let ctx = squint_core::contract::EstimateContext {
        binder: squint_core::contract::Binder {
            version: "test",
            config_identity: None,
            session_id: None,
        },
        config: Some(&config),
        tools: &t,
        readiness: table_manners::Readiness {
            state: table_manners::ReadinessState::Ready,
            requirements: Vec::new(),
        },
        pricing: Some(&refusing),
        session: None,
        now_ms: 0,
        ttl_ms: 1,
    };
    let err =
        squint_core::contract::estimate(&ctx, "source_fidelity", &job, &root(), None).unwrap_err();
    assert_eq!(err.class(), "image_limit");
}
