//! Task 7.8: a decision's observer sees every journal line as it is written, in order, with the
//! run directory it belongs to, so a server can turn steps into progress.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::run;
use squint_core::tool::{Envelope, Tool, ToolRegistry};
use squint_testkit::{ScriptedProvider, ScriptedTool};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_observer_sees_every_journal_line_in_order() {
    let out = std::env::temp_dir().join(format!("squint-observer-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let text = std::fs::read_to_string(root().join("fixtures/config/fake-tool.toml")).unwrap();
    let config = Config::parse(&text, &[], &|_| None).unwrap();
    let provider = ScriptedProvider::from_path(&root().join("fixtures/scripts/held.json")).unwrap();
    let envelopes: Vec<Envelope> = serde_json::from_str(
        &std::fs::read_to_string(root().join("fixtures/tool-scripts/empty.json")).unwrap(),
    )
    .unwrap();
    let tools: Vec<Box<dyn Tool>> = vec![Box::new(ScriptedTool::fake_regions(envelopes))];
    let tools = ToolRegistry::new(tools, &["fake_regions".into()], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let seen: Arc<Mutex<Vec<(PathBuf, String)>>> = Arc::default();
    let sink = seen.clone();
    let observer: run::Observer = Arc::new(move |run_dir: &Path, event: &str| {
        sink.lock()
            .unwrap()
            .push((run_dir.to_path_buf(), event.to_string()));
    });
    let services = run::Services::new(&provider, &tools).with_observer(observer);
    let outcome = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run::check(&config, &job, &root(), &services, &out, "test"))
        .unwrap();
    let journal: Vec<String> = std::fs::read_to_string(outcome.dir.join("run-1/journal.jsonl"))
        .unwrap()
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.iter().map(|(_, e)| e.clone()).collect::<Vec<_>>(),
        journal
    );
    assert!(
        seen.iter()
            .all(|(dir, _)| *dir == outcome.dir.join("run-1"))
    );
}
