//! `[screen]`: the tools a rules layer names run before the model's first turn, with the
//! arguments they default to; their results and regions open the conversation and do not spend
//! the model's tool budget. A tool that cannot be given its arguments is reported, not run.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::provider::{ContentBlock, TurnResponse};
use squint_core::run;
use squint_core::tool::{Envelope, Manifest, Tool, ToolRegistry};
use squint_testkit::{FAKE_REGIONS_MANIFEST, ScriptedProvider, ScriptedTool};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-screen-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(overrides: &[&str]) -> Config {
    let text = std::fs::read_to_string(root().join("fixtures/config/fake-tool.toml")).unwrap();
    let extra = root().join("fixtures/rules/screen-fake.toml");
    let text = format!("{text}\n[rules]\nbase = \"source-fidelity\"\nextra = [{extra:?}]\n");
    let ov: Vec<String> = overrides.iter().map(|s| s.to_string()).collect();
    Config::parse(&text, &ov, &|_| None).unwrap()
}

fn envelopes() -> Vec<Envelope> {
    let text = std::fs::read_to_string(root().join("fixtures/tool-scripts/regions-1-then-1.json"))
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// fake_regions, its image argument described with a default the screen can fill.
fn tool(description: &str) -> ToolRegistry {
    let manifest = Manifest::from_toml(&FAKE_REGIONS_MANIFEST.replace(
        "description = \"The image id to find changes in.\"",
        &format!("description = {description:?}"),
    ))
    .unwrap();
    let t: Vec<Box<dyn Tool>> = vec![Box::new(ScriptedTool::with_manifest(manifest, envelopes()))];
    ToolRegistry::new(t, &["fake_regions".into()], &[]).unwrap()
}

fn item(id: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id, "disposition": "held", "confidence": "confident", "reason": "scripted",
        "allowed_by": null, "location": null, "evidence": []
    })
}

fn submit(items: &[&str]) -> ScriptedProvider {
    let turn: TurnResponse = serde_json::from_value(serde_json::json!({
        "tool_calls": [{
            "id": "c_submit",
            "name": "submit_verdict",
            "arguments": {
                "schema_version": 1,
                "items": items.iter().map(|i| item(i)).collect::<Vec<_>>(),
                "findings": [],
                "opinion": { "verdict": "pass", "reason": "r" }
            }
        }],
        "stop_reason": "tool_use"
    }))
    .unwrap();
    ScriptedProvider::new(vec![turn])
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn texts(blocks: &[ContentBlock]) -> Vec<String> {
    blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn the_screen_opens_the_conversation_and_its_regions_are_on_the_checklist() {
    let out = tempdir("opens");
    let tools = tool("The image id to find changes in, usually the candidate.");
    let provider = submit(&["chk:ob:SF1@candidate", "chk:ob:SF2@candidate", "chk:ev:1:0"]);
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&["budget.tool_calls=1"]),
        &job,
        &root(),
        &run::Services::new(&provider, &tools),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);

    let recorded = provider.recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1, "the model submitted on its first turn");
    let first = texts(&recorded[0].history[0].content);
    let at = |needle: &str| {
        first
            .iter()
            .position(|t| t.contains(needle))
            .unwrap_or_else(|| panic!("no {needle:?} in {first:#?}"))
    };
    let preamble = at("Before your first turn the harness ran `fake_regions`");
    let result = at("Screen: `fake_regions` with {\"image\":\"candidate\"}");
    let checklist = at("chk:ob:SF1@candidate");
    assert!(
        preamble < result && result < checklist,
        "the screen comes before the checklist"
    );
    assert!(
        first[checklist].contains("chk:ev:1:0"),
        "the checklist carries the screen's region: {}",
        first[checklist]
    );
    assert!(
        first[result].contains("scripted regions"),
        "{}",
        first[result]
    );

    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(
        report["rules"]["screen"],
        serde_json::json!(["fake_regions"])
    );
    let run = &report["runs"][0];
    assert_eq!(run["tool_calls"].as_array().unwrap().len(), 1);
    // The screen spent none of the model's budget of one call: the model could still call.
    assert!(
        recorded[0]
            .history
            .iter()
            .flat_map(|m| texts(&m.content))
            .any(|t| t.contains("at most 1 calls")),
        "the instruction still offers the full budget"
    );
}

#[test]
fn a_tool_the_screen_cannot_give_arguments_is_reported_not_run() {
    let out = tempdir("no-default");
    let tools = tool("The image id to find changes in.");
    let provider = submit(&["chk:ob:SF1@candidate", "chk:ob:SF2@candidate"]);
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &run::Services::new(&provider, &tools),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let recorded = provider.recorded.lock().unwrap();
    let first = texts(&recorded[0].history[0].content);
    assert!(
        first
            .iter()
            .any(|t| t.contains("Screen: `fake_regions` was not run")),
        "{first:#?}"
    );
}
