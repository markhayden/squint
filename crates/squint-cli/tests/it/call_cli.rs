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

// Task 10.5b: `squint call --bundle <path>` (table-manners 4.8, 4.9).

/// The fixture images as bytes: candidate, source.
fn pair_bytes() -> (Vec<u8>, Vec<u8>) {
    (
        std::fs::read(images().join("candidate-64x48.png")).unwrap(),
        std::fs::read(images().join("source-64x48.png")).unwrap(),
    )
}

/// Serves each call in `calls` in one session on the scripted provider (`held.json`), the images
/// copied into `base/original`, the only `--allow-dir`, and runs written to `base/served`; returns
/// each result.
fn served_runs(base: &Path, calls: &[(&str, Value)]) -> Vec<Value> {
    let original = base.join("original");
    std::fs::create_dir_all(&original).unwrap();
    for name in ["candidate-64x48.png", "source-64x48.png"] {
        std::fs::copy(images().join(name), original.join(name)).unwrap();
    }
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
        .arg(&original);
    let mut client = Client::start(&mut server).unwrap();
    calls
        .iter()
        .map(|(action, arguments)| {
            let mut arguments = arguments.clone();
            for pointer in ["/candidate", "/source", "/references/0", "/references/1"] {
                if let Some(Value::Object(image)) = arguments.pointer_mut(pointer) {
                    let name = Path::new(image["path"].as_str().unwrap())
                        .file_name()
                        .unwrap()
                        .to_owned();
                    image.insert("path".into(), json!(original.join(name)));
                }
            }
            client.call(action, arguments).unwrap().structured.unwrap()
        })
        .collect()
}

/// A bundle of one step per result, its manifest and members (the testkit writer).
fn bundle_of(results: &[Value]) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    let (candidate, source) = pair_bytes();
    let mut steps = Vec::new();
    let mut members = Vec::new();
    for r in results {
        let (step, m) = squint_testkit::bundle::step_from_run(r, &[&candidate, &source]);
        steps.push(step);
        members.extend(m);
    }
    (squint_testkit::bundle::manifest(steps), members)
}

/// `squint call --bundle <bundle>` from `cwd`, every path absolute: the minimal config, `script`,
/// runs under `base/runs`, squint's cache at `base/cache`; `extra` follows.
fn replay_from(base: &Path, cwd: &Path, bundle: &Path, script: &str, extra: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(cwd)
        .env("XDG_CACHE_HOME", base.join("cache"))
        .args(["call", "--bundle"])
        .arg(bundle)
        .arg("--config")
        .arg(repo_root().join(MINIMAL))
        .arg("--provider-script")
        .arg(repo_root().join(script))
        .arg("--out")
        .arg(base.join("runs"))
        .args(extra)
        .output()
        .unwrap()
}

/// Every file under `dir`, relative, sorted.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.push(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// The replay of 4.9: a bundle written from a served check, moved to a fresh directory with the
/// original run directory and input files deleted, replays offline on the scripted provider from
/// another working directory, as the archive and as the unpacked directory, and records the same
/// invocation, input identity and decision. It writes only under its runs root and squint's
/// cache (the inputs under `bundles/<sha256 of invocation.json>/inputs/`), never beside the bundle
/// or in the working directory, and a second replay reuses the placed inputs.
#[test]
fn a_bundle_replays_offline_with_the_original_gone() {
    let base = scratch("bundle-replay");
    let served = served_runs(
        &base,
        &[(
            "source_fidelity",
            json!({
                "candidate": image("candidate-64x48.png"),
                "source": image("source-64x48.png"),
                "brief": "Keep the structure.",
                "max_spend_usd": 0.5,
            }),
        )],
    )
    .remove(0);
    assert_eq!(served["status"], "completed", "{served}");
    let (manifest, members) = bundle_of(std::slice::from_ref(&served));
    let moved = base.join("moved");
    std::fs::create_dir_all(&moved).unwrap();
    let archive = moved.join("bundle.tar");
    std::fs::write(
        &archive,
        squint_testkit::bundle::canonical(&manifest, &members),
    )
    .unwrap();
    let unpacked = moved.join("unpacked");
    squint_testkit::bundle::write_dir(&unpacked, &manifest, &members);
    std::fs::remove_dir_all(base.join("original")).unwrap();
    std::fs::remove_dir_all(base.join("served")).unwrap();
    let elsewhere = base.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let before = files_under(&moved);
    let manifest_sha = squint_core::bundle::Bundle::read(&archive)
        .unwrap()
        .manifest_sha256;

    for (n, bundle) in [&archive, &unpacked, &archive].into_iter().enumerate() {
        let o = replay_from(
            &base,
            &elsewhere,
            bundle,
            "fixtures/scripts/held.json",
            &["--allow-dir", moved.to_str().unwrap()],
        );
        assert_eq!(
            o.status.code(),
            Some(0),
            "{n}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let r = stdout_json(&o);
        valid(RESULT, &r);
        assert_eq!(r["status"], "completed", "{n}");
        assert_eq!(r["invocation"], served["invocation"], "{n}");
        assert_eq!(r["input_identity"], served["input_identity"], "{n}");
        assert_eq!(r["body"]["verdict"], served["body"]["verdict"], "{n}");
        assert_eq!(r["images"]["in"], served["images"]["in"], "{n}");
        assert!(r["run"]["session_id"].is_null(), "{n}: a command-line run");
        let dir = PathBuf::from(r["run"]["dir"].as_str().unwrap());
        assert!(dir.starts_with(base.join("runs")), "{n}: {dir:?}");
        assert_eq!(run_dirs(&base.join("runs")).len(), n + 1, "{n}");
    }
    assert!(files_under(&elsewhere).is_empty(), "the working directory");
    assert_eq!(files_under(&moved), before, "beside the bundle");
    let mut top: Vec<String> = std::fs::read_dir(&base)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    top.sort();
    assert_eq!(top, ["cache", "elsewhere", "moved", "runs"]);
    let mut placed: Vec<PathBuf> = members
        .iter()
        .map(|(name, _)| PathBuf::from(format!("squint/bundles/{manifest_sha}/{name}")))
        .collect();
    placed.sort();
    assert_eq!(files_under(&base.join("cache")), placed);
}

/// What `call --bundle` refuses before any step, each exit 64 with the reason on stderr, nothing
/// on stdout, no run, no provider call and nothing placed in the cache: a bundle outside every
/// `--allow-dir` (and any bundle with none), a step another tool made, an input left out
/// (`reference_only`), an input member whose bytes are not its `file_sha256` (archive and
/// directory), and inputs that are not the images the invocation names.
#[cfg(unix)]
#[test]
fn a_bundle_squint_cannot_replay_is_refused_before_any_step() {
    let base = scratch("bundle-refused");
    let served = served_runs(&base, &[("source_fidelity", pair())]).remove(0);
    let (manifest, members) = bundle_of(std::slice::from_ref(&served));
    let edited = |edit: &dyn Fn(&mut Value)| {
        let mut doc: Value = serde_json::from_slice(&manifest).unwrap();
        edit(&mut doc);
        serde_json::to_vec_pretty(&doc).unwrap()
    };
    let allowed = base.join("allowed");
    let outside = base.join("outside");
    std::fs::create_dir_all(&allowed).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let write = |dir: &Path, name: &str, manifest: &[u8], members: &[(String, Vec<u8>)]| {
        let path = dir.join(name);
        std::fs::write(&path, squint_testkit::bundle::canonical(manifest, members)).unwrap();
        path
    };
    let good = write(&allowed, "good.tar", &manifest, &members);
    let escaping = allowed.join("escaping.tar");
    std::os::unix::fs::symlink(write(&outside, "out.tar", &manifest, &members), &escaping).unwrap();
    let mut tampered = members.clone();
    let last = tampered[0].1.len() - 1;
    tampered[0].1[last] ^= 1;
    let tampered_dir = allowed.join("tampered-dir");
    squint_testkit::bundle::write_dir(&tampered_dir, &manifest, &tampered);
    let swapped = edited(&|m| {
        let inputs = m["steps"][0]["inputs"].as_array_mut().unwrap();
        inputs.swap(0, 1);
        inputs[0]["slot"] = json!("candidate");
        inputs[1]["slot"] = json!("source");
    });
    let allow = allowed.to_str().unwrap();
    let cases: Vec<(&str, PathBuf, Vec<&str>, &str)> = vec![
        (
            "outside",
            write(&outside, "o.tar", &manifest, &members),
            vec!["--allow-dir", allow],
            "is a path outside the directories this session allows",
        ),
        (
            "symlink-out",
            escaping,
            vec!["--allow-dir", allow],
            "is a path outside the directories this session allows",
        ),
        (
            "missing",
            allowed.join("no-such.tar"),
            vec!["--allow-dir", allow],
            "is a path outside the directories this session allows",
        ),
        (
            "no-allow-dir",
            good.clone(),
            vec![],
            "is a path outside the directories this session allows",
        ),
        (
            "other-tool",
            write(
                &allowed,
                "rascal.tar",
                &edited(&|m| m["steps"][0]["tool"]["name"] = json!("rascal")),
                &members,
            ),
            vec!["--allow-dir", allow],
            "steps[0] is a call to rascal; squint replays only its own steps",
        ),
        (
            "reference-only",
            write(
                &allowed,
                "left-out.tar",
                &edited(&|m| {
                    m["steps"][0]["reference_only"] =
                        json!([{ "slot": "references", "why": "the person left it out" }]);
                }),
                &members,
            ),
            vec!["--allow-dir", allow],
            "steps[0] left references out (reference_only)",
        ),
        (
            "tampered-archive",
            write(&allowed, "tampered.tar", &manifest, &tampered),
            vec!["--allow-dir", allow],
            "not the file_sha256 it is listed with",
        ),
        (
            "tampered-dir",
            tampered_dir,
            vec!["--allow-dir", allow],
            "not the file_sha256 it is listed with",
        ),
        (
            "inputs-not-the-invocation",
            write(&allowed, "swapped.tar", &swapped, &members),
            vec!["--allow-dir", allow],
            "steps[0].inputs[0] is not the image the invocation's candidate expects",
        ),
    ];
    let elsewhere = base.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    for (tag, bundle, extra, want) in cases {
        let case = base.join(tag);
        std::fs::create_dir_all(&case).unwrap();
        let o = replay_from(
            &case,
            &elsewhere,
            &bundle,
            "fixtures/scripts/held.json",
            &extra,
        );
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(64), "{tag}: {stderr}");
        assert!(stderr.contains(want), "{tag}: want {want:?} in {stderr}");
        assert!(stderr.contains("invalid (input)"), "{tag}: {stderr}");
        assert!(o.stdout.is_empty(), "{tag}: no result");
        assert!(run_dirs(&case.join("runs")).is_empty(), "{tag}: no run");
        assert!(!case.join("cache").exists(), "{tag}: nothing placed");
    }
    // The refusal of a path says nothing about the file: the same words for one that exists
    // outside and one that does not.
    let words = |bundle: &Path| {
        let case = base.join("words");
        let o = replay_from(
            &case,
            &elsewhere,
            bundle,
            "fixtures/scripts/held.json",
            &["--allow-dir", allow],
        );
        String::from_utf8_lossy(&o.stderr).replace(bundle.to_str().unwrap(), "<bundle>")
    };
    assert_eq!(
        words(&outside.join("o.tar")),
        words(&outside.join("absent.tar"))
    );
    // `--bundle` takes no action and no `--arguments`.
    for extra in [
        vec!["source_fidelity"],
        vec!["--arguments", "fixtures/does-not-exist.json"],
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_squint"))
            .current_dir(repo_root())
            .args(["call", "--bundle"])
            .arg(&good)
            .args(extra)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(64));
        assert!(o.stdout.is_empty());
    }
}

/// A bundle of several steps prints one result per line, in order, each step a run of its own;
/// it stops at the first step that does not complete and exits with that step's code. A step that
/// completes with a failing verdict is still completed: the replay goes on, and when every step
/// completes it exits with the most serious step's code, so an earlier `fail` (or `unsure`) is not
/// hidden by a later `pass`.
///
/// Each step reads the provider script afresh, so the steps differ by their checklists: `check`
/// has items SF1 and SF2, `required` adds RQ1. The `requirement-held-then-*` scripts open with a
/// passing verdict over all three, which `required` takes; `check` is told its RQ1 is unknown and
/// takes the second turn, a `fail` (a violation of SF2) or an `unsure` (SF1 can't tell).
#[test]
fn several_steps_print_a_line_each_and_stop_at_the_first_that_does_not_complete() {
    let base = scratch("bundle-steps");
    let mut with_requirement = pair();
    with_requirement["requirements"] = json!([{
        "id": "RQ1",
        "kind": "must_hold",
        "scope": ["candidate"],
        "text": "The sky is kept.",
    }]);
    let served = served_runs(
        &base,
        &[
            ("source_fidelity", pair()),
            (
                "reference_consistency",
                json!({
                    "candidate": image("candidate-64x48.png"),
                    "references": [image("source-64x48.png"), image("candidate-64x48.png")],
                }),
            ),
            ("source_fidelity", with_requirement),
        ],
    );
    let (check, refs, required) = (&served[0], &served[1], &served[2]);
    assert_eq!(check["status"], "completed", "{check}");
    assert_eq!(refs["status"], "error", "{refs}");
    assert!(
        required["invocation"]["arguments"]["requirements"].is_array(),
        "{required}"
    );
    let unknown = {
        let mut r = check.clone();
        r["invocation"]["action"] = json!("no_such_check");
        r
    };
    let allowed = base.join("allowed");
    std::fs::create_dir_all(&allowed).unwrap();
    let elsewhere = base.join("elsewhere");
    std::fs::create_dir_all(&elsewhere).unwrap();
    // (tag, steps, script, exit code, the invocation of each line printed)
    type Case<'a> = (&'a str, Vec<&'a Value>, &'a str, i32, Vec<&'a Value>);
    let cases: Vec<Case> = vec![
        (
            "all-pass",
            vec![check, check],
            "fixtures/scripts/held.json",
            0,
            vec![check, check],
        ),
        (
            "all-fail",
            vec![check, check],
            "fixtures/scripts/violation.json",
            1,
            vec![check, check],
        ),
        (
            "fail-then-pass",
            vec![check, required],
            "fixtures/scripts/requirement-held-then-violation.json",
            1,
            vec![check, required],
        ),
        (
            "unsure-then-pass",
            vec![check, required],
            "fixtures/scripts/requirement-held-then-cant-tell.json",
            2,
            vec![check, required],
        ),
        (
            "stops-at-error",
            vec![check, refs, check],
            "fixtures/scripts/held.json",
            3,
            vec![check, refs],
        ),
        (
            "stops-at-unknown-action",
            vec![check, &unknown, check],
            "fixtures/scripts/held.json",
            64,
            vec![check],
        ),
    ];
    for (tag, steps, script, code, printed) in cases {
        let steps: Vec<Value> = steps.into_iter().cloned().collect();
        let (manifest, members) = bundle_of(&steps);
        let bundle = allowed.join(format!("{tag}.tar"));
        std::fs::write(
            &bundle,
            squint_testkit::bundle::canonical(&manifest, &members),
        )
        .unwrap();
        let case = base.join(tag);
        let o = replay_from(
            &case,
            &elsewhere,
            &bundle,
            script,
            &["--allow-dir", allowed.to_str().unwrap()],
        );
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(code), "{tag}: {stderr}");
        let stdout = String::from_utf8(o.stdout).unwrap();
        let lines: Vec<Value> = stdout
            .lines()
            .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{tag}: {e}: {l}")))
            .collect();
        assert_eq!(lines.len(), printed.len(), "{tag}: {stdout}");
        for (line, want) in lines.iter().zip(&printed) {
            valid(RESULT, line);
            if code < 3 {
                assert_eq!(line["status"], "completed", "{tag}: {line}");
            }
            assert_eq!(line["invocation"], want["invocation"], "{tag}");
            assert_eq!(line["input_identity"], want["input_identity"], "{tag}");
        }
        assert_eq!(
            run_dirs(&case.join("runs")).len(),
            printed.len(),
            "{tag}: one run per step that ran"
        );
    }
}
