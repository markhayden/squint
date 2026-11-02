//! Task 11.2: `--must-hold`, `--must-not` and `--allow` become the job's requirements `RQ1`, `RQ2`,
//! … exactly as `[[job.requirements]]` in a job file would: in `--dry-run`, in the verdict, in the
//! cache key and on `estimate`; beside a job file that has its own, they are refused.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-intent-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn squint(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(args)
        .output()
        .unwrap()
}

fn report(out: &Path) -> serde_json::Value {
    let dir = std::fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap()
}

const CONFIG: [&str; 2] = ["--config", "fixtures/config/minimal.toml"];

#[test]
fn the_flags_are_job_requirements_in_the_dry_run() {
    let mut args = vec!["check"];
    args.extend(CONFIG);
    args.extend([
        "--candidate",
        "fixtures/images/candidate-64x48.png",
        "--source",
        "fixtures/images/source-64x48.png",
        "--use-case",
        "source-fidelity",
        "--allow",
        "the sky may change colour.",
        "--must-not",
        "a white patch is added.",
        "--must-hold",
        "the bridge keeps its arches.",
        "--dry-run",
    ]);
    let o = squint(&args);
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        stdout.contains(
            "RQ1 (must hold; judges candidate; consults source) the bridge keeps its arches."
        ) && stdout.contains(
            "RQ2 (must not occur; judges candidate; consults source) a white patch is added."
        ) && stdout.contains("RQ3 (allowed; on candidate) the sky may change colour."),
        "{stdout}"
    );
    assert!(
        stdout.contains("RQ1@candidate kind=MustHold") && stdout.contains("[job]"),
        "{stdout}"
    );
    assert!(stdout.contains("RQ2@candidate kind=MustNot"), "{stdout}");
    assert!(stdout.contains("chk:ob:RQ2@candidate"), "{stdout}");
}

#[test]
fn a_must_not_the_model_marks_violated_fails_the_check() {
    let out = tempdir("violation");
    let mut args = vec!["check"];
    args.extend(CONFIG);
    args.extend([
        "--job",
        "fixtures/jobs/pair.toml",
        "--must-not",
        "a white patch is added",
        "--provider-script",
        "fixtures/scripts/intent-must-not-violation.json",
        "--out",
        out.to_str().unwrap(),
    ]);
    let o = squint(&args);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{stderr}");
    assert_eq!(String::from_utf8_lossy(&o.stdout), "fail\n");
    assert!(
        stderr.contains("why: fail: RQ1@candidate violated (confident)"),
        "{stderr}"
    );
    let r = report(&out);
    let rq1 = r["rules"]["obligations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "RQ1")
        .unwrap()
        .clone();
    assert_eq!(rq1["kind"], "must_not");
    assert_eq!(rq1["provenance"], "job");
    assert_eq!(rq1["text"], "a white patch is added");
}

/// The flags and the same requirement in a job file make the same decision: the second check is a
/// cache hit of the first.
#[test]
fn the_flags_and_the_same_job_file_share_a_cache_key() {
    let base = tempdir("cache");
    let cache = format!("cache.dir={}", base.join("cache").display());
    let run = |out: &str, job: &[&str]| {
        let out = base.join(out);
        let mut args = vec!["check"];
        args.extend(CONFIG);
        args.extend(["--set", &cache]);
        args.extend(job);
        args.extend([
            "--provider-script",
            "fixtures/scripts/intent-must-not-violation.json",
            "--out",
            out.to_str().unwrap(),
            "--quiet",
        ]);
        let o = squint(&args);
        assert_eq!(
            o.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        report(&out)
    };
    let first = run(
        "out-flags",
        &[
            "--job",
            "fixtures/jobs/pair.toml",
            "--must-not",
            "a white patch is added",
        ],
    );
    assert_eq!(first["cache"]["status"], "miss");
    let again = run("out-file", &["--job", "fixtures/jobs/pair-must-not.toml"]);
    assert_eq!(again["cache"]["status"], "hit", "{}", again["cache"]);
}

#[test]
fn a_job_file_with_its_own_requirements_refuses_the_flags() {
    let mut args = vec!["check"];
    args.extend(CONFIG);
    args.extend([
        "--job",
        "fixtures/jobs/pair-must-not.toml",
        "--must-hold",
        "the bridge keeps its arches",
        "--dry-run",
    ]);
    let o = squint(&args);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(64), "{stderr}");
    assert!(
        stderr.contains("fixtures/jobs/pair-must-not.toml has its own [[job.requirements]]"),
        "{stderr}"
    );
    assert!(o.stdout.is_empty());
}

#[test]
fn estimate_takes_the_flags() {
    let estimate = |extra: &[&str]| -> f64 {
        let mut args = vec!["estimate", "--json"];
        args.extend(CONFIG);
        args.extend([
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            "fixtures/scripts/held.json",
        ]);
        args.extend(extra);
        let o = squint(&args);
        assert_eq!(
            o.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&o.stderr)
        );
        let e: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        e["worst_case_usd"].as_f64().unwrap()
    };
    let plain = estimate(&[]);
    let stated = estimate(&["--must-not", "a white patch is added"]);
    assert!(
        stated > plain,
        "a requirement adds a checklist item to every planned request: {stated} vs {plain}"
    );
}
