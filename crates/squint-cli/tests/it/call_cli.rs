//! Task 10.4: `squint call <action> --arguments <file>` (table-manners 4.8): any action from
//! exactly the arguments an MCP call to it would send, under the path rule with the directories
//! `--allow-dir` gives (5.2), a command-line run with its `run.json`, the result on stdout and the
//! exit codes of 6. A served run's invocation, with a `path` locator added to each image, run
//! through `call` is the same call.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use squint_testkit::mcp_client::Client;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// A fresh scratch directory for one test.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-call-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn images() -> PathBuf {
    repo_root().join("fixtures/images")
}

fn image(name: &str) -> Value {
    json!({ "path": images().join(name) })
}

fn pair() -> Value {
    json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") })
}

/// The configuration every call here uses unless a test writes its own.
const MINIMAL: &str = "fixtures/config/minimal.toml";

/// `squint call <action>` with `arguments` written to a file under `base`, `config` and a
/// provider script, runs written to `base/runs`; `extra` follows.
fn call_with(
    base: &Path,
    action: &str,
    arguments: &Value,
    script: &str,
    config: &str,
    extra: &[&str],
) -> Command {
    let file = base.join("args.json");
    std::fs::write(&file, arguments.to_string()).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_squint"));
    c.current_dir(repo_root())
        .args(["call", action, "--arguments"])
        .arg(&file)
        .args(["--config", config, "--provider-script", script, "--out"])
        .arg(base.join("runs"))
        .args(extra);
    c
}

/// `call_with`, allowed the fixture images' directory, run to its end.
fn call(base: &Path, action: &str, arguments: &Value, script: &str) -> Output {
    let allow = images();
    call_with(
        base,
        action,
        arguments,
        script,
        MINIMAL,
        &["--allow-dir", allow.to_str().unwrap()],
    )
    .output()
    .unwrap()
}

fn stdout_json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not a result ({e}): {}\nstderr: {}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    })
}

/// The contract's result schema, by id.
const RESULT: &str = "urn:table-manners:core:result";

/// A document valid against one of the contract's schemas, by id.
fn valid(id: &str, doc: &Value) {
    struct Embedded;
    impl jsonschema::Retrieve for Embedded {
        fn retrieve(
            &self,
            uri: &jsonschema::Uri<String>,
        ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
            let (_, text) = table_manners::SCHEMAS
                .iter()
                .find(|(id, _)| *id == uri.as_str())
                .ok_or("unknown schema")?;
            Ok(serde_json::from_str(text)?)
        }
    }
    let (_, text) = table_manners::SCHEMAS
        .iter()
        .find(|(schema, _)| *schema == id)
        .unwrap();
    let schema: Value = serde_json::from_str(text).unwrap();
    let validator = jsonschema::options()
        .with_retriever(Embedded)
        .build(&schema)
        .unwrap();
    let errors: Vec<String> = validator.iter_errors(doc).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{id}: {errors:?}\n{doc}");
}

/// The run directories under `runs`.
fn run_dirs(runs: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(runs)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default()
}

/// `squint check` of the same pair, its exit code.
fn check_exit(base: &Path, script: &str) -> Option<i32> {
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/minimal.toml",
            "--use-case",
            "source-fidelity",
            "--candidate",
            "fixtures/images/candidate-64x48.png",
            "--source",
            "fixtures/images/source-64x48.png",
            "--provider-script",
            script,
            "--out",
        ])
        .arg(base.join("check-runs"))
        .output()
        .unwrap()
        .status
        .code()
}

/// The result on stdout validates against `result.json`, equals the `run.json` it wrote, is a
/// command-line run (no session, no `client_run_id`), and `call` exits as `check` does for the
/// same pair: 0 pass, 1 fail, 3 error.
#[test]
fn call_prints_the_result_and_exits_as_check_does() {
    for (n, (script, want)) in [
        ("fixtures/scripts/held.json", 0),
        ("fixtures/scripts/violation.json", 1),
        ("fixtures/scripts/missing-item.json", 3),
    ]
    .into_iter()
    .enumerate()
    {
        let base = scratch(&format!("exit-{n}"));
        let o = call(&base, "source_fidelity", &pair(), script);
        assert_eq!(
            o.status.code(),
            Some(want),
            "{script}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert_eq!(check_exit(&base, script), Some(want), "{script}");
        let r = stdout_json(&o);
        valid(RESULT, &r);
        assert_eq!(r["action"], "source_fidelity");
        assert!(r["run"]["session_id"].is_null(), "{r}");
        assert!(r["run"]["client_run_id"].is_null(), "{r}");
        let dir = PathBuf::from(r["run"]["dir"].as_str().unwrap());
        assert!(dir.starts_with(base.join("runs")), "{dir:?}");
        let on_disk: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join("run.json")).unwrap()).unwrap();
        assert_eq!(on_disk, r);
    }
}

/// The mapping is mechanical (table-manners 4.8): a served run's `invocation.arguments`, each
/// image given back a `path` beside its recorded `expect`, run through `squint call
/// <invocation.action>` record the same invocation, the same input identity and the same
/// decision on the scripted provider: a completed check with a brief and a spend cap, and one
/// with `references` in order (the scripted verdict does not fit its rules, so both runs end in
/// the same `error`, which is still a decision to repeat).
#[test]
fn a_served_invocation_with_locators_is_the_same_call() {
    let base = scratch("replay");
    let cases = [
        (
            "source_fidelity",
            json!({
                "candidate": image("candidate-64x48.png"),
                "source": image("source-64x48.png"),
                "brief": "Keep the structure.",
                "max_spend_usd": 0.5,
                "return_images": true,
            }),
            vec![
                ("/candidate", "candidate-64x48.png"),
                ("/source", "source-64x48.png"),
            ],
            ("completed", 0),
        ),
        (
            "reference_consistency",
            json!({
                "candidate": image("candidate-64x48.png"),
                "references": [image("source-64x48.png"), image("candidate-64x48.png")],
            }),
            vec![
                ("/candidate", "candidate-64x48.png"),
                ("/references/0", "source-64x48.png"),
                ("/references/1", "candidate-64x48.png"),
            ],
            ("error", 3),
        ),
    ];
    for (action, arguments, images_at, (status, code)) in cases {
        let mut server = Command::new(env!("CARGO_BIN_EXE_squint"));
        server
            .current_dir(repo_root())
            .args([
                "serve",
                "--config",
                MINIMAL,
                "--provider-script",
                "fixtures/scripts/held.json",
                "--out",
            ])
            .arg(base.join("served"))
            .arg("--allow-dir")
            .arg(images());
        let mut client = Client::start(&mut server).unwrap();
        let served = client.call(action, arguments).unwrap().structured.unwrap();
        assert_eq!(served["status"], status, "{served}");
        let invocation = &served["invocation"];
        assert_eq!(invocation["action"], action);
        let mut replayed = invocation["arguments"].clone();
        for (pointer, name) in &images_at {
            let slot = replayed
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap();
            assert!(slot.contains_key("expect"), "{action}{pointer}");
            slot.insert("path".into(), json!(images().join(name)));
        }
        let case = base.join(action);
        std::fs::create_dir_all(&case).unwrap();
        let o = call(&case, action, &replayed, "fixtures/scripts/held.json");
        assert_eq!(
            o.status.code(),
            Some(code),
            "{action}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let r = stdout_json(&o);
        assert_eq!(r["status"], status, "{action}");
        assert_eq!(&r["invocation"], invocation, "{action}");
        assert_eq!(r["input_identity"], served["input_identity"], "{action}");
        assert_eq!(r["body"]["verdict"], served["body"]["verdict"], "{action}");
        assert_eq!(r["images"]["in"], served["images"]["in"], "{action}");
    }
}

/// The path rule (table-manners 5.2) as `serve` applies it: a path outside the allowed
/// directories, and any path with no `--allow-dir`, is an `invalid` run, `input`, exit 64, with
/// the served refusal's words; `[serve] allow_dirs` narrows the flags.
#[test]
fn a_refused_path_is_invalid_input_in_the_words_serve_uses() {
    let base = scratch("refused");
    let outside = base.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let path = outside.join("candidate-64x48.png");
    std::fs::copy(images().join("candidate-64x48.png"), &path).unwrap();
    let arguments = json!({ "candidate": { "path": path }, "source": image("source-64x48.png") });
    let served_reason = {
        let mut server = Command::new(env!("CARGO_BIN_EXE_squint"));
        server
            .current_dir(repo_root())
            .args([
                "serve",
                "--config",
                "fixtures/config/minimal.toml",
                "--provider-script",
                "fixtures/scripts/held.json",
                "--out",
            ])
            .arg(base.join("served"))
            .arg("--allow-dir")
            .arg(images());
        let mut client = Client::start(&mut server).unwrap();
        let r = client
            .call("source_fidelity", arguments.clone())
            .unwrap()
            .structured
            .unwrap();
        r["error"]["reason"].clone()
    };
    let allow = images();
    let config = base.join("narrow.toml");
    std::fs::write(
        &config,
        format!(
            "{}\n[serve]\nallow_dirs = [{:?}]\n",
            std::fs::read_to_string(repo_root().join("fixtures/config/minimal.toml")).unwrap(),
            base.join("outside").display().to_string()
        ),
    )
    .unwrap();
    let cases: [(&str, Value, &str, Vec<&str>); 3] = [
        (
            "outside",
            arguments.clone(),
            MINIMAL,
            vec!["--allow-dir", allow.to_str().unwrap()],
        ),
        ("no-allow-dir", pair(), MINIMAL, vec![]),
        (
            "narrowed",
            pair(),
            config.to_str().unwrap(),
            vec!["--allow-dir", allow.to_str().unwrap()],
        ),
    ];
    for (tag, args, config, extra) in cases {
        let case = base.join(tag);
        std::fs::create_dir_all(&case).unwrap();
        let o = call_with(
            &case,
            "source_fidelity",
            &args,
            "fixtures/scripts/held.json",
            config,
            &extra,
        )
        .output()
        .unwrap();
        assert_eq!(
            o.status.code(),
            Some(64),
            "{tag}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let r = stdout_json(&o);
        valid(RESULT, &r);
        assert_eq!(
            (&r["status"], &r["error"]["code"]),
            (&json!("invalid"), &json!("input")),
            "{tag}: {r}"
        );
        assert!(r["invocation"].is_null(), "{tag}: {r}");
        assert_eq!(r["spend"]["usd"], 0.0);
        if tag == "outside" {
            assert_eq!(r["error"]["reason"], served_reason, "{tag}");
        } else {
            let reason = r["error"]["reason"].as_str().unwrap();
            assert!(
                reason.contains("a path outside the directories this session allows"),
                "{tag}: {reason}"
            );
        }
        let dir = PathBuf::from(r["run"]["dir"].as_str().unwrap());
        assert!(
            dir.join("run.json").exists(),
            "{tag}: a refused path is a run"
        );
    }
}

/// What `call` refuses before any run, each exit 64 with nothing written: an unknown action, a
/// reserved name, arguments that are not a JSON object, and an argument `job_from_call` refuses
/// (an unknown key, an empty `references`, no candidate).
#[test]
fn a_bad_action_or_bad_arguments_exits_64_before_any_run() {
    let base = scratch("usage");
    let cases = [
        ("unknown-action", "no_such_check", pair()),
        ("reserved-export", "export", pair()),
        ("reserved-estimate", "estimate", pair()),
        ("reserved-describe", "describe", pair()),
        ("not-an-object", "source_fidelity", json!(["candidate"])),
        (
            "unknown-argument",
            "source_fidelity",
            json!({ "candidate": image("candidate-64x48.png"), "colour": "red" }),
        ),
        (
            "empty-references",
            "reference_consistency",
            json!({ "candidate": image("candidate-64x48.png"), "references": [] }),
        ),
        ("no-candidate", "source_fidelity", json!({ "brief": "x" })),
    ];
    let allow = images();
    for (tag, action, arguments) in cases {
        let case = base.join(tag);
        std::fs::create_dir_all(&case).unwrap();
        let o = call_with(
            &case,
            action,
            &arguments,
            "fixtures/scripts/held.json",
            MINIMAL,
            &["--allow-dir", allow.to_str().unwrap()],
        )
        .output()
        .unwrap();
        assert_eq!(
            o.status.code(),
            Some(64),
            "{tag}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        assert!(o.stdout.is_empty(), "{tag}: no result");
        assert!(run_dirs(&case.join("runs")).is_empty(), "{tag}: no run");
    }
    // A missing arguments file, and a relative `--allow-dir`, are usage errors too.
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "call",
            "source_fidelity",
            "--arguments",
            "fixtures/does-not-exist.json",
            "--config",
            "fixtures/config/minimal.toml",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(64));
    let case = base.join("relative-allow-dir");
    std::fs::create_dir_all(&case).unwrap();
    let o = call_with(
        &case,
        "source_fidelity",
        &pair(),
        "fixtures/scripts/held.json",
        MINIMAL,
        &["--allow-dir", "fixtures/images"],
    )
    .output()
    .unwrap();
    assert_eq!(o.status.code(), Some(64));
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("--allow-dir fixtures/images"), "{stderr}");
}

/// SIGINT and SIGTERM cancel a call as they do a check (table-manners 6): exit 130 or 143, and
/// the result on stdout is the `cancelled` run.
#[cfg(unix)]
fn killed_with(signal: &str, code: i32) {
    let base = scratch(&format!("signal-{signal}"));
    let allow = images();
    let child = call_with(
        &base,
        "source_fidelity",
        &pair(),
        "fixtures/scripts/slow-retry.json",
        MINIMAL,
        &["--allow-dir", allow.to_str().unwrap()],
    )
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    // Wait until the run is sleeping before its retry.
    let started = Instant::now();
    let retrying = || {
        run_dirs(&base.join("runs")).iter().any(|d| {
            std::fs::read_to_string(d.join("run-1/journal.jsonl"))
                .is_ok_and(|j| j.contains("\"event\":\"retry\""))
        })
    };
    while !retrying() {
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the run never reached the retry"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let status = Command::new("kill")
        .args([format!("-{signal}"), child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let output = child.wait_with_output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let r = stdout_json(&output);
    assert_eq!(r["status"], "cancelled", "{r}");
}

#[cfg(unix)]
#[test]
fn sigterm_cancels_a_call_with_143() {
    killed_with("TERM", 143);
}

#[cfg(unix)]
#[test]
fn sigint_cancels_a_call_with_130() {
    killed_with("INT", 130);
}
