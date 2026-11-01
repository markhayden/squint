//! Task 1.10b: the report is derived from the journal, so a rebuild from every truncation point
//! has the right terminal status and the full rebuild equals the report `check` wrote; the schema
//! is current; prune respects age.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::report::{self, Report};
use squint_core::run::{self, Cancel, Services};
use squint_core::tool::ToolRegistry;
use squint_testkit::ScriptedProvider;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-report-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config() -> Config {
    let text = std::fs::read_to_string(root().join("fixtures/config/minimal.toml")).unwrap();
    Config::parse(&text, &[], &|_| None).unwrap()
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn check(tag: &str, script: &str) -> run::Outcome {
    let out = tempdir(tag);
    let provider =
        ScriptedProvider::from_path(&root().join("fixtures/scripts").join(script)).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    block_on(run::check(
        &config(),
        &job,
        &root(),
        &Services::new(&provider, &tools),
        &out,
        "test",
    ))
    .unwrap()
}

/// A copy of a decision directory whose run-1 journal holds only the first `keep` lines.
fn truncated_copy(dir: &Path, keep: usize, tag: &str) -> PathBuf {
    let copy = tempdir(tag).join("decision-1-1");
    std::fs::create_dir_all(copy.join("run-1")).unwrap();
    std::fs::copy(dir.join("decision.json"), copy.join("decision.json")).unwrap();
    let journal = std::fs::read_to_string(dir.join("run-1/journal.jsonl")).unwrap();
    let kept: Vec<&str> = journal.lines().take(keep).collect();
    let mut text = kept.join("\n");
    if !kept.is_empty() {
        text.push('\n');
    }
    std::fs::write(copy.join("run-1/journal.jsonl"), text).unwrap();
    copy
}

#[test]
fn report_rebuild_from_every_truncation_point_has_the_right_terminal_status() {
    let o = check("truncate", "held.json");
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let journal = std::fs::read_to_string(o.dir.join("run-1/journal.jsonl")).unwrap();
    let lines: Vec<&str> = journal.lines().collect();
    assert!(lines.len() >= 6, "{}", lines.len());
    for keep in 0..=lines.len() {
        let copy = truncated_copy(&o.dir, keep, &format!("truncate-{keep}"));
        let r = Report::rebuild(&copy).unwrap();
        let run = &r.runs[0];
        let sent = lines[..keep]
            .iter()
            .filter(|l| l.contains("\"event\":\"turn_sent\""))
            .count();
        assert_eq!(run.turns.len(), sent, "keep {keep}");
        if keep == lines.len() {
            assert_eq!(run.terminal, report::TERMINAL_VERDICT);
            assert_eq!(r.exit_code, 0);
            assert!(run.error.is_none());
        } else {
            assert_eq!(run.terminal, report::TERMINAL_INCOMPLETE, "keep {keep}");
            assert_eq!(run.error.as_ref().unwrap().class, "incomplete");
            assert!(r.verdict.is_none() && r.decision.error);
            assert_eq!(r.exit_code, 3);
        }
    }
}

#[test]
fn report_rebuild_equals_the_report_check_wrote() {
    let o = check("equal", "held.json");
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    let rebuilt = serde_json::to_value(Report::rebuild(&o.dir).unwrap()).unwrap();
    assert_eq!(written, rebuilt);
    assert!(o.dir.join("decision.json").exists());
}

#[test]
fn report_rebuild_of_a_failed_run_keeps_its_error_class() {
    let o = check("failed", "missing-item.json");
    let r = Report::rebuild(&o.dir).unwrap();
    assert_eq!(r.runs[0].terminal, report::TERMINAL_ERROR);
    assert_eq!(
        r.runs[0].error.as_ref().unwrap().class,
        "submission_invalid"
    );
    assert_eq!(r.runs[0].repairs.len(), 1);
}

#[test]
fn report_rebuild_of_a_cancelled_run_is_cancelled() {
    let out = tempdir("cancelled");
    let provider =
        ScriptedProvider::from_path(&root().join("fixtures/scripts/slow-retry.json")).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let cancel = Cancel::new();
    let config = config();
    let base = root();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let o = runtime.block_on(async {
        let services = Services::new(&provider, &tools).with_cancel(&cancel);
        let run = run::check(&config, &job, &base, &services, &out, "test");
        let trigger = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        };
        let (o, ()) = tokio::join!(run, trigger);
        o.unwrap()
    });
    let r = Report::rebuild(&o.dir).unwrap();
    assert_eq!(r.runs[0].terminal, report::TERMINAL_CANCELLED);
    assert_eq!(r.runs[0].error.as_ref().unwrap().class, "cancelled");
    assert_eq!(r.runs[0].turns.len(), 1);
    assert!(
        r.runs[0].turns[0].response.is_none(),
        "no response came back"
    );
    assert!(!r.runs[0].turns[0].entered_history);
    assert_eq!(
        r.runs[0].turns[0].attempts, 2,
        "the first attempt and the retry about to be made"
    );
}

#[test]
fn report_schema_is_current() {
    let generated = serde_json::to_string_pretty(&Report::json_schema()).unwrap() + "\n";
    let path = root().join("schemas/report-v1.json");
    if std::env::var_os("UPDATE_SCHEMAS").is_some() {
        std::fs::write(&path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        committed, generated,
        "schemas/report-v1.json is stale; run with UPDATE_SCHEMAS=1 to regenerate"
    );
}

#[test]
fn report_prune_respects_age() {
    let out = tempdir("prune");
    let now = SystemTime::now();
    let ms = |ago: Duration| (now - ago).duration_since(UNIX_EPOCH).unwrap().as_millis();
    let old = out.join(format!("decision-{}-1", ms(Duration::from_secs(3 * 3600))));
    let young = out.join(format!("decision-{}-2", ms(Duration::from_secs(60))));
    let other = out.join("notes");
    for d in [&old, &young, &other] {
        std::fs::create_dir_all(d.join("run-1")).unwrap();
    }
    let removed = report::prune(&out, Duration::from_secs(3600), now).unwrap();
    assert_eq!(removed, std::slice::from_ref(&old));
    assert!(!old.exists() && young.exists() && other.exists());
    let removed = report::prune(&out, Duration::ZERO, now).unwrap();
    assert_eq!(removed, std::slice::from_ref(&young));
    assert!(other.exists(), "only decision directories are pruned");
}

#[test]
fn report_rebuild_from_every_truncation_point_of_a_journal_with_tools_repairs_and_retries() {
    // A run with tool calls, a stray final call repaired, and the tool script's drops.
    let out = tempdir("rich");
    let text = std::fs::read_to_string(root().join("fixtures/tool-scripts/regions-3-then-1.json"))
        .unwrap();
    let envelopes: Vec<squint_core::tool::Envelope> = serde_json::from_str(&text).unwrap();
    let tool: Vec<Box<dyn squint_core::tool::Tool>> = vec![Box::new(
        squint_testkit::ScriptedTool::fake_regions(envelopes),
    )];
    let tools = ToolRegistry::new(tool, &["fake_regions".into()], &[]).unwrap();
    let provider =
        ScriptedProvider::from_path(&root().join("fixtures/scripts/stray-then-submit.json"))
            .unwrap();
    let config_text =
        std::fs::read_to_string(root().join("fixtures/config/fake-tool.toml")).unwrap();
    let config = Config::parse(&config_text, &["budget.tool_turns=1".to_string()], &|_| {
        None
    })
    .unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config,
        &job,
        &root(),
        &Services::new(&provider, &tools),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let journal = std::fs::read_to_string(o.dir.join("run-1/journal.jsonl")).unwrap();
    let lines: Vec<&str> = journal.lines().collect();
    assert!(
        journal.contains("\"event\":\"repair\"") && journal.contains("\"event\":\"tool_called\"")
    );
    for keep in 0..=lines.len() {
        let copy = truncated_copy(&o.dir, keep, &format!("rich-{keep}"));
        let r = Report::rebuild(&copy).unwrap();
        let run = &r.runs[0];
        if keep == lines.len() {
            assert_eq!(run.terminal, report::TERMINAL_VERDICT);
            assert_eq!(run.repairs.len(), 1);
            assert_eq!(
                run.tool_calls.len(),
                2,
                "the rejected call and the refused stray call"
            );
        } else {
            assert_eq!(run.terminal, report::TERMINAL_INCOMPLETE, "keep {keep}");
            assert_eq!(r.exit_code, 3);
        }
        let sent = lines[..keep]
            .iter()
            .filter(|l| l.contains("\"event\":\"turn_sent\""))
            .count();
        assert_eq!(run.turns.len(), sent, "keep {keep}");
    }
}

// ---- added at the M3 baseline: checklist-item violations carry their root location ---------

#[test]
fn a_checklist_item_violation_is_mapped_to_its_root_image() {
    let o = check("item-root", "violation.json");
    assert_eq!(o.exit_code, 1, "{:?}", o.error);
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    let items = r["runs"][0]["items"].as_array().unwrap();
    let violation = items
        .iter()
        .find(|i| i["disposition"] == "violation")
        .expect("the scripted submission marks SF2 a violation");
    assert_eq!(violation["id"], "chk:ob:SF2@candidate");
    // The location is on the raw frame of the candidate, so its root is the candidate itself,
    // in the same coordinates; the scorer and the viewer read `root` without mapping anything.
    assert_eq!(violation["root"]["image"], "candidate");
    assert_eq!(violation["root"]["frame"], "raw:candidate");
    assert_eq!(violation["root"]["kind"], "whole_image");
    assert_eq!(violation["root"]["partial"], false);
    // A held item with no location has no root either.
    let held = items.iter().find(|i| i["disposition"] == "held").unwrap();
    assert!(held.get("root").is_none(), "{held}");
}
