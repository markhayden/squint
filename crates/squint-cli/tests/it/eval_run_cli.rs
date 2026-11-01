//! Tasks 3.4b, 3.4c, 3.5a and 3.5b at the binary: `eval run` over the fixture set with the
//! scripted provider, resuming with nothing redone; `eval estimate`; `eval report` byte for byte;
//! `eval gate` over a synthetic sealed blind set.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn eval_run_completes_a_fake_matrix_and_resumes_without_redoing_it() {
    let out = std::env::temp_dir().join(format!("squint-eval-run-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_squint"))
            .current_dir(repo_root())
            .args([
                "eval",
                "run",
                "--matrix",
                "fixtures/matrix/fake-a.toml",
                "--out",
                out.to_str().unwrap(),
                "--evalsets-root",
                "fixtures/evalsets",
                "--provider-script",
                "fixtures/scripts/held.json",
            ])
            .output()
            .unwrap()
    };
    let o = run();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(
        text.contains("4 decisions (4 completed now, 0 already complete"),
        "{text}"
    );
    assert!(out.join("summary.json").exists() && out.join("descriptor.json").exists());
    let o = run();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(
        text.contains("(0 completed now, 4 already complete"),
        "{text}"
    );
    // The run wrote the Markdown report; `eval report` reproduces it byte for byte.
    let stored = std::fs::read(out.join("report.md")).unwrap();
    assert!(!stored.is_empty());
    std::fs::remove_file(out.join("report.md")).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "eval",
            "report",
            "--out",
            out.to_str().unwrap(),
            "--evalsets-root",
            "fixtures/evalsets",
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(std::fs::read(out.join("report.md")).unwrap(), stored);
    // The results document generator assembles the published write-up from the directory.
    let doc = out.join("doc");
    let o = Command::new("python3")
        .current_dir(repo_root())
        .args([
            "-I",
            "scripts/results-doc.py",
            out.to_str().unwrap(),
            "fixtures/matrix/fake-a.toml",
            doc.to_str().unwrap(),
            "--title",
            "the scripted check",
            "--bin",
            env!("CARGO_BIN_EXE_squint"),
        ])
        .env("SQUINT_EVALSETS_ROOT", "fixtures/evalsets")
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let readme = std::fs::read_to_string(doc.join("README.md")).unwrap();
    for needle in [
        "# fake-a: the scripted check",
        "**These are development sets.**",
        "## Development sets",
        "## Adversarial sets",
        "| fake:fake-model/A | scripted |",
        "| Detected failures |",
    ] {
        assert!(readme.contains(needle), "{needle}: {readme}");
    }
    assert!(
        doc.join("summary.json").is_file()
            && doc.join("metrics.json").is_file()
            && doc.join("matrix.toml").is_file()
    );
    // The published summary names decision directories relative to the evaluation directory.
    let published: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(doc.join("summary.json")).unwrap()).unwrap();
    for d in published["decisions"].as_array().unwrap() {
        let dir = d["dir"].as_str().unwrap();
        assert!(dir.starts_with("results/"), "{dir}");
    }
    // A subset of sets prints to stdout and writes nothing; an unknown set is refused.
    let before = std::fs::metadata(out.join("report.md"))
        .unwrap()
        .modified()
        .unwrap();
    let o = squint(
        &repo_root(),
        &[
            "eval",
            "report",
            "--out",
            out.to_str().unwrap(),
            "--evalsets-root",
            "fixtures/evalsets",
            "--sets",
            "source-fidelity/pairs-v1",
        ],
    );
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(o.stdout, stored, "one set is the whole matrix here");
    assert_eq!(
        std::fs::metadata(out.join("report.md"))
            .unwrap()
            .modified()
            .unwrap(),
        before
    );
    let o = squint(
        &repo_root(),
        &[
            "eval",
            "report",
            "--out",
            out.to_str().unwrap(),
            "--evalsets-root",
            "fixtures/evalsets",
            "--sets",
            "no/such",
        ],
    );
    assert_eq!(o.status.code(), Some(64));
    // A matrix whose config is missing is a usage error.
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "eval",
            "run",
            "--matrix",
            "fixtures/matrix/no-such.toml",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(64));
}

#[test]
fn eval_estimate_prints_every_envelope_and_warns_over_the_cap() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "eval",
            "estimate",
            "--matrix",
            "fixtures/matrix/fake-a.toml",
            "--evalsets-root",
            "fixtures/evalsets",
            "--provider-script",
            "fixtures/scripts/held.json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(
        text.contains("4 decisions")
            && text.contains("envelope sum $")
            && !text.contains("warning:"),
        "{text}"
    );
    // A matrix whose envelopes sum past its cap: the warning, and exit 0 all the same.
    let tight =
        std::env::temp_dir().join(format!("squint-estimate-cli-{}.toml", std::process::id()));
    let matrix = std::fs::read_to_string(repo_root().join("fixtures/matrix/fake-a.toml"))
        .unwrap()
        .replace("spend_cap_usd = 5.0", "spend_cap_usd = 0.1");
    std::fs::write(&tight, matrix).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "eval",
            "estimate",
            "--matrix",
            tight.to_str().unwrap(),
            "--evalsets-root",
            "fixtures/evalsets",
            "--provider-script",
            "fixtures/scripts/held.json",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let e: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(e["over_cap"], true);
    assert_eq!(e["decisions"].as_array().unwrap().len(), 4);
}

fn squint(cwd: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn eval_gate_scores_a_blind_set_writes_the_receipt_and_publishes_the_labels() {
    let dir = std::env::temp_dir().join(format!("squint-eval-gate-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let evalsets = dir.join("evalsets");
    let set = "source-fidelity/blind-cli-v1";
    let set_dir = squint_testkit::synthetic_set(
        &evalsets,
        &repo_root().join("fixtures/evalsets/source-fidelity/pairs-v1/items"),
        set,
        "blind",
        "cli",
        7,
    )
    .unwrap();
    let owner = dir.join("owner");
    let o = squint(
        &repo_root(),
        &[
            "sets",
            "seal",
            "--set",
            set_dir.to_str().unwrap(),
            "--attest",
            "no judge has seen these",
            "--labels-out",
            owner.to_str().unwrap(),
        ],
    );
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let matrix = dir.join("matrix.toml");
    std::fs::write(
        &matrix,
        format!(
            "[matrix]\nname = \"gate-cli\"\nconfig = \"fixtures/config/minimal.toml\"\nsets = [{set:?}]\narms = [\"C\", \"D_fake_regions\"]\ndecision_runs = 1\nrepetitions = 1\nspend_cap_usd = 50.0\n\n[matrix.gate]\nseed = 3\nresamples = 1000\n\n[[matrix.models]]\nprofile = \"fake\"\nmodel = \"fake-model\"\n"
        ),
    )
    .unwrap();
    let out = dir.join("eval");
    // Arms C and D_<tool> need `view` and `align`, which arrive in M4, so the binary cannot run
    // this matrix yet; the evaluation directory is produced in process with a fake that runs the
    // control arm without them. One script for every decision: both arms fail every item, so the
    // tool detects nothing beyond the control and the gate rejects it; the lifecycle still runs
    // end to end through the binary.
    {
        use squint_eval::runner::{Factories, Plan};
        struct Fake;
        impl Factories for Fake {
            fn provider(
                &self,
                _cell: &squint_eval::matrix::Cell,
                _config: &squint_core::config::Config,
            ) -> squint_eval::Result<Box<dyn squint_core::provider::Provider>> {
                Ok(Box::new(
                    squint_testkit::ScriptedProvider::from_path(
                        &repo_root().join("fixtures/scripts/violation.json"),
                    )
                    .unwrap(),
                ))
            }
            fn tools(
                &self,
                selection: &squint_eval::arms::ToolSelection,
                _config: &squint_core::config::Config,
            ) -> squint_eval::Result<squint_core::tool::ToolRegistry> {
                let tool = squint_testkit::ScriptedTool::fake_regions_from_path(
                    &repo_root().join("fixtures/tool-scripts/regions-25.json"),
                )
                .unwrap();
                let enabled: Vec<String> = selection
                    .enabled
                    .iter()
                    .filter(|n| *n == "fake_regions")
                    .cloned()
                    .collect();
                Ok(
                    squint_core::tool::ToolRegistry::new(vec![Box::new(tool)], &enabled, &[])
                        .unwrap(),
                )
            }
        }
        let matrix_text = std::fs::read_to_string(&matrix).unwrap();
        let parsed = squint_eval::matrix::Matrix::parse(&matrix_text).unwrap();
        let config_text = std::fs::read_to_string(repo_root().join(&parsed.matrix.config)).unwrap();
        let plan = Plan {
            eval_dir: out.clone(),
            matrix: parsed,
            matrix_text,
            config_text,
            evalsets_root: evalsets.clone(),
            admissions: None,
            harness_version: "0.1.0 (test)".into(),
            allow_fake: true,
        };
        let s = squint_eval::runner::run(&plan, &Fake, None).unwrap();
        assert_eq!(s.decisions.len(), 42, "21 items × 2 cells");
    }
    let gate = |extra: &[&str]| {
        let mut args = vec![
            "eval",
            "gate",
            "--matrix",
            matrix.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--tool",
            "fake_regions",
            "--set",
            set,
            "--stage",
            "initial",
            "--evalsets-root",
            evalsets.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        squint(&repo_root(), &args)
    };
    let o = gate(&[]);
    assert_eq!(o.status.code(), Some(64), "a blind set needs --labels");
    let labels = owner.join("labels.toml");
    let o = gate(&["--labels", labels.to_str().unwrap()]);
    let text = String::from_utf8_lossy(&o.stdout);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        text.contains("(21 items): rejected") && text.contains("admission: unchanged"),
        "{text}"
    );
    assert!(
        text.contains("gate not held: detected failures: not higher than C"),
        "{text}"
    );
    assert!(text.contains("review and commit"), "{text}");
    assert!(evalsets.join("receipts").read_dir().unwrap().count() == 1);
    assert!(set_dir.join("labels.toml").exists());
    assert!(
        std::fs::read_to_string(set_dir.join("set.toml"))
            .unwrap()
            .contains("split = \"heldout\"")
    );
    assert!(
        !evalsets.join("admissions.toml").exists(),
        "a rejection records nothing"
    );
    // Again: resumed from the receipt, as JSON.
    let o = gate(&["--labels", labels.to_str().unwrap(), "--json"]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["resumed"], true);
    assert_eq!(v["receipt"]["receipt"]["outcome"], "rejected");
}

#[test]
fn eval_requalify_records_a_baseline_and_eval_canary_compares_against_it() {
    let dir = std::env::temp_dir().join(format!("squint-canary-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state = dir.join("canary.toml");
    let out = dir.join("out");
    let run = |command: &str, script: &str, extra: &[&str]| {
        let mut args = vec![
            "eval",
            command,
            "--config",
            "fixtures/config/minimal.toml",
            "--set",
            "source-fidelity/pairs-v1",
            "--evalsets-root",
            "fixtures/evalsets",
            "--state",
            state.to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
            "--provider-script",
            script,
        ];
        args.extend_from_slice(extra);
        squint(&repo_root(), &args)
    };
    let o = run("canary", "fixtures/scripts/held.json", &[]);
    assert_eq!(o.status.code(), Some(64), "no baseline yet");
    let o = run("requalify", "fixtures/scripts/held.json", &[]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("not recorded"));
    assert!(!state.exists());
    let o = run(
        "requalify",
        "fixtures/scripts/held.json",
        &["--approve", "owner: qualified"],
    );
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("recorded in"));
    let o = run("canary", "fixtures/scripts/held.json", &[]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains(": clear (0 deviation(s)"));
    let o = run("canary", "fixtures/scripts/violation.json", &["--json"]);
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["status"], "suspect");
    assert_eq!(v["deviations"].as_array().unwrap().len(), 2);
    let o = run("canary", "fixtures/scripts/violation.json", &[]);
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(
        text.contains(": recommend_disable") && text.contains("nothing was disabled"),
        "{text}"
    );
    // A spend cap that cannot cover one item refuses before anything is sent.
    let o = run(
        "canary",
        "fixtures/scripts/held.json",
        &["--spend-cap-usd", "0.001"],
    );
    assert_eq!(o.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&o.stderr).contains("spend cap"));
}

#[test]
fn a_relative_output_directory_still_gives_relative_decision_directories_and_the_doc_relativises_old_ones()
 {
    // The regions gate run was started with a relative --out and its summary carried absolute
    // decision directories (the run makes them absolute for the packs). Both forms must end up
    // relative: in the runner's summary, and in the published copy of an older summary.
    let rel = format!("target/eval-run-cli-rel-{}", std::process::id());
    let out = repo_root().join(&rel);
    let _ = std::fs::remove_dir_all(&out);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "eval",
            "run",
            "--matrix",
            "fixtures/matrix/fake-a.toml",
            "--out",
            &rel,
            "--evalsets-root",
            "fixtures/evalsets",
            "--provider-script",
            "fixtures/scripts/held.json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let summary_path = out.join("summary.json");
    let mut summary: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&summary_path).unwrap()).unwrap();
    for d in summary["decisions"].as_array().unwrap() {
        let dir = d["dir"].as_str().unwrap();
        assert!(dir.starts_with("results/"), "{dir}");
    }
    // An older summary with absolute directories: the published copy is relative.
    for d in summary["decisions"].as_array_mut().unwrap() {
        let dir = d["dir"].as_str().unwrap().to_string();
        d["dir"] = serde_json::Value::String(out.join(dir).display().to_string());
    }
    std::fs::write(
        &summary_path,
        serde_json::to_string_pretty(&summary).unwrap(),
    )
    .unwrap();
    let doc = out.join("doc");
    let o = Command::new("python3")
        .current_dir(repo_root())
        .args([
            "-I",
            "scripts/results-doc.py",
            &rel,
            "fixtures/matrix/fake-a.toml",
            doc.to_str().unwrap(),
            "--title",
            "the scripted check",
            "--bin",
            env!("CARGO_BIN_EXE_squint"),
        ])
        .env("SQUINT_EVALSETS_ROOT", "fixtures/evalsets")
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let published: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(doc.join("summary.json")).unwrap()).unwrap();
    for d in published["decisions"].as_array().unwrap() {
        let dir = d["dir"].as_str().unwrap();
        assert!(dir.starts_with("results/"), "{dir}");
    }
    let _ = std::fs::remove_dir_all(&out);
}
