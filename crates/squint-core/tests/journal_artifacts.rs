//! Task 1.10a: every artifact a journal event references exists with matching content and was
//! written before its event (crash test with an injected fault); no secret reaches any artifact;
//! cancellation appends `cancelled` with the state reached.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use sha2::Digest;
use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::provider::{StopReason, TurnResponse};
use squint_core::run::{self, Cancel, Services};
use squint_core::tool::ToolRegistry;
use squint_testkit::ScriptedProvider;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-artifacts-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(file: &str, env: &dyn Fn(&str) -> Option<String>) -> Config {
    let text = std::fs::read_to_string(root().join("fixtures/config").join(file)).unwrap();
    Config::parse(&text, &[], env).unwrap()
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

fn journal(dir: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join("run-1/journal.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

/// Every artifact reference (an object with `sha256`, `bytes` and `truncated`) an event mentions;
/// the image entries' own file hashes are not references.
fn referenced(value: &serde_json::Value, out: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(m) => {
            if let (Some(serde_json::Value::String(s)), Some(_), Some(_)) =
                (m.get("sha256"), m.get("bytes"), m.get("truncated"))
            {
                out.push(s.clone());
            }
            for v in m.values() {
                referenced(v, out);
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|v| referenced(v, out)),
        _ => {}
    }
}

#[test]
fn journal_artifacts_referenced_by_events_exist_with_matching_content() {
    let out = tempdir("refs");
    let provider = ScriptedProvider::from_path(&root().join("fixtures/scripts/held.json")).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config("minimal.toml", &|_| None),
        &job,
        &root(),
        &Services::new(&provider, &tools),
        &out,
        "test",
    ))
    .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let mut refs = Vec::new();
    for e in journal(&o.dir) {
        referenced(&e["data"], &mut refs);
    }
    assert!(
        refs.len() >= 4,
        "two images, a request and a response: {refs:?}"
    );
    for sha in &refs {
        let path = o.dir.join("run-1/artifacts").join(sha);
        let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(&squint_core::image::hex(&sha2::Sha256::digest(&bytes)), sha);
    }
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    let run = &report["runs"][0];
    assert_eq!(run["images_sent"].as_object().unwrap().len(), 2);
    let request = run["turns"][0]["request"]["sha256"].as_str().unwrap();
    let body = std::fs::read_to_string(o.dir.join("run-1/artifacts").join(request)).unwrap();
    assert!(body.contains("\"max_output_tokens\"") && body.contains("\"system\""));
}

#[test]
fn journal_artifacts_are_written_before_their_event_even_when_the_append_dies() {
    let out = tempdir("fault");
    let provider = ScriptedProvider::from_path(&root().join("fixtures/scripts/held.json")).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let mut services = Services::new(&provider, &tools);
    // resolved 0, prepared 1, reserved 2, turn_sent 3: die between the request artifact and its event.
    services.fail_journal_at = Some(3);
    let err = block_on(run::check(
        &config("minimal.toml", &|_| None),
        &job,
        &root(),
        &services,
        &out,
        "test",
    ))
    .unwrap_err();
    assert!(err.to_string().contains("injected journal fault"), "{err}");
    let decision = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let lines = std::fs::read_to_string(decision.join("run-1/journal.jsonl")).unwrap();
    assert_eq!(lines.lines().count(), 3, "the event never landed");
    let artifacts: Vec<PathBuf> = std::fs::read_dir(decision.join("run-1/artifacts"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(
        artifacts.len(),
        3,
        "two images and the request body: {artifacts:?}"
    );
    assert!(artifacts.iter().any(|p| {
        std::fs::read_to_string(p)
            .map(|t| t.contains("\"max_output_tokens\""))
            .unwrap_or(false)
    }));
    assert!(
        !artifacts
            .iter()
            .any(|p| p.extension().is_some_and(|e| e == "tmp")),
        "no temp file left"
    );
    assert!(provider.remaining() == 1, "the request was never sent");
}

#[test]
fn journal_artifacts_hold_no_secret_even_when_the_model_echoes_it() {
    let secret = "SYNTHETIC-squint-account-7f3e9a";
    let out = tempdir("secret");
    let config = config("secret.toml", &|name| {
        (name == "SQUINT_TEST_ACCOUNT").then(|| secret.to_string())
    });
    assert_eq!(config.secrets, [secret]);
    let valid: Vec<TurnResponse> = serde_json::from_str(
        &std::fs::read_to_string(root().join("fixtures/scripts/held.json")).unwrap(),
    )
    .unwrap();
    let echo = TurnResponse {
        text: vec![format!("The account is {secret}.")],
        tool_calls: vec![],
        content: vec![],
        stop_reason: StopReason::EndTurn,
        usage: Default::default(),
        provider_request_id: None,
    };
    let provider = ScriptedProvider::new(vec![echo, valid[0].clone()]);
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
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
    let mut files = vec![
        o.dir.join("report.json"),
        o.dir.join("run.json"),
        o.dir.join("run-1/journal.jsonl"),
    ];
    files.extend(
        std::fs::read_dir(o.dir.join("run-1/artifacts"))
            .unwrap()
            .map(|e| e.unwrap().path()),
    );
    let mut saw_redaction = false;
    for f in files {
        let bytes = std::fs::read(&f).unwrap();
        assert!(
            !bytes.windows(secret.len()).any(|w| w == secret.as_bytes()),
            "secret in {}",
            f.display()
        );
        if bytes.windows(10).any(|w| w == b"[redacted]") {
            saw_redaction = true;
        }
    }
    assert!(saw_redaction, "the echoed secret was stored redacted");
}

#[test]
fn journal_cancel_during_a_retry_wait_appends_cancelled_with_the_state_reached() {
    let out = tempdir("cancel");
    let provider =
        ScriptedProvider::from_path(&root().join("fixtures/scripts/slow-retry.json")).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let cancel = Cancel::new();
    let config = config("minimal.toml", &|_| None);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let base = root();
    let o = runtime.block_on(async {
        let services = Services::new(&provider, &tools).with_cancel(&cancel);
        let run = run::check(&config, &job, &base, &services, &out, "test");
        let trigger = async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cancel.cancel();
        };
        let (o, ()) = tokio::join!(run, trigger);
        o.unwrap()
    });
    assert_eq!(o.exit_code, 3);
    let ev = journal(&o.dir);
    let last = ev.last().unwrap();
    assert_eq!(last["event"], "cancelled");
    assert_eq!(
        last["data"]["state"], "retry",
        "cancelled while sleeping before the retry"
    );
    assert_eq!(last["data"]["class"], "cancelled");
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["runs"][0]["error"]["class"], "cancelled");
    assert!(report["verdict"].is_null());
}

// ---- added at the M1 checkpoint from the code review ----------------------------------------

#[test]
fn journal_cancel_in_the_first_run_stops_a_two_run_decision() {
    let out = tempdir("cancel-two-runs");
    let provider =
        ScriptedProvider::from_path(&root().join("fixtures/scripts/slow-retry.json")).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let cancel = Cancel::new();
    let text = std::fs::read_to_string(root().join("fixtures/config/minimal.toml")).unwrap();
    let config = Config::parse(&text, &["budget.decision_runs=2".to_string()], &|_| None).unwrap();
    let base = root();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let o = runtime.block_on(async {
        let services = Services::new(&provider, &tools).with_cancel(&cancel);
        let run = run::check(&config, &job, &base, &services, &out, "test");
        let trigger = async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            cancel.cancel();
        };
        let (o, ()) = tokio::join!(run, trigger);
        o.unwrap()
    });
    assert_eq!(o.exit_code, 3);
    assert!(
        o.dir.join("run-1").exists() && !o.dir.join("run-2").exists(),
        "run 2 never started"
    );
    let report: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(report["decision"]["error"], true);
    assert_eq!(report["runs"].as_array().unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn journal_every_run_directory_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let out = tempdir("modes");
    let provider = ScriptedProvider::from_path(&root().join("fixtures/scripts/held.json")).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config("minimal.toml", &|_| None),
        &job,
        &root(),
        &Services::new(&provider, &tools),
        &out,
        "test",
    ))
    .unwrap();
    let mut files = vec![
        o.dir.join("report.json"),
        o.dir.join("decision.json"),
        o.dir.join("run-1/journal.jsonl"),
    ];
    for sub in ["run-1/artifacts", "run-1/frames"] {
        files.extend(
            std::fs::read_dir(o.dir.join(sub))
                .unwrap()
                .map(|e| e.unwrap().path()),
        );
    }
    assert!(files.len() >= 7, "{files:?}");
    for f in files {
        let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", f.display());
    }
}
