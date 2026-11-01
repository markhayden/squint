//! Task 1.3 CLI scenarios: `--set` overrides show in the report, `cloud_calls = false` refuses, an
//! expanded secret appears in no output path, and the shorthand job equals the job file.
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

fn squint() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_squint"));
    c.current_dir(repo_root());
    c
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-config-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_dir(out: &Path) -> PathBuf {
    std::fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap()
}

fn report(out: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(run_dir(out).join("report.json")).unwrap())
        .unwrap()
}

fn base_args(out: &Path) -> Vec<String> {
    vec![
        "check".into(),
        "--config".into(),
        "fixtures/config/minimal.toml".into(),
        "--provider-script".into(),
        "fixtures/scripts/held.json".into(),
        "--out".into(),
        out.to_str().unwrap().into(),
    ]
}

#[test]
fn set_override_changes_a_single_value_and_shows_in_the_report() {
    let out = tempdir("override");
    let mut args = base_args(&out);
    args.extend([
        "--job".into(),
        "fixtures/jobs/pair.toml".into(),
        "--set".into(),
        "budget.tool_turns=2".into(),
    ]);
    let o = squint().args(&args).output().unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let r = report(&out);
    assert_eq!(r["config"]["budget"]["tool_turns"], 2);
    assert_eq!(
        r["config"]["budget"]["tool_calls"], 8,
        "other defaults untouched"
    );
}

#[test]
fn cloud_calls_false_refuses_with_the_documented_message() {
    let out = tempdir("optin");
    let mut args = base_args(&out);
    args.extend([
        "--job".into(),
        "fixtures/jobs/pair.toml".into(),
        "--set".into(),
        "privacy.cloud_calls=false".into(),
    ]);
    let o = squint().args(&args).output().unwrap();
    assert_eq!(o.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&o.stderr).contains("cloud_calls must be true"));
}

#[test]
fn an_expanded_secret_appears_in_no_output_path() {
    let out = tempdir("secret");
    let secret = "org-s3cret-value-7781";
    let o: Output = squint()
        .env("SQUINT_TEST_ACCOUNT", secret)
        .args([
            "check",
            "--config",
            "fixtures/config/secret.toml",
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            "fixtures/scripts/held.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let stdout = String::from_utf8_lossy(&o.stdout);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(!stdout.contains(secret) && !stderr.contains(secret));
    let dir = run_dir(&out);
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let text = std::fs::read_to_string(&path).unwrap();
            assert!(
                !text.contains(secret),
                "{} leaks the secret",
                path.display()
            );
        }
    }
    let r = report(&out);
    assert_eq!(r["config"]["privacy"]["account"], "[redacted]");
}

#[test]
fn shorthand_job_equals_the_job_file() {
    let out_a = tempdir("shorthand-a");
    let mut a = base_args(&out_a);
    a.extend(["--job".into(), "fixtures/jobs/pair.toml".into()]);
    assert_eq!(squint().args(&a).output().unwrap().status.code(), Some(0));

    let out_b = tempdir("shorthand-b");
    let mut b = base_args(&out_b);
    b.extend([
        "--use-case".into(),
        "source-fidelity".into(),
        "--candidate".into(),
        "fixtures/images/candidate-64x48.png".into(),
        "--source".into(),
        "fixtures/images/source-64x48.png".into(),
        "--brief".into(),
        "Restyle the scene; keep the structure.".into(),
    ]);
    let o = squint().args(&b).output().unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );

    let ra = report(&out_a);
    let rb = report(&out_b);
    assert_eq!(
        ra["job"], rb["job"],
        "resolved jobs differ:\n{}\n{}",
        ra["job"], rb["job"]
    );
    assert_eq!(ra["runs"][0]["images"], rb["runs"][0]["images"]);
}

#[test]
fn a_reference_in_shorthand_gets_a_ref_id_and_label() {
    let out = tempdir("shorthand-ref");
    let mut b = base_args(&out);
    b.extend([
        "--use-case".into(),
        "reference-consistency".into(),
        "--candidate".into(),
        "fixtures/images/candidate-64x48.png".into(),
        "--ref".into(),
        "pip=fixtures/images/source-64x48.png".into(),
    ]);
    let o = squint().args(&b).output().unwrap();
    // The walking slice's hard-coded obligation still applies; exit code is not the point here.
    assert!(
        o.status.code().is_some(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let r = report(&out);
    let imgs = r["job"]["images"].as_array().unwrap();
    assert!(
        imgs.iter()
            .any(|i| i["id"] == "ref:pip" && i["role"] == "reference" && i["label"] == "pip")
    );
}

/// `check` cannot run without a configuration (task 9.2): with none on the lookup it exits 64
/// naming where squint looked, and writes nothing. `estimate` answers like `describe` (task 9.4):
/// refused for readiness, with nothing priced and the configuration's identity null.
#[test]
fn check_without_a_config_names_the_lookup() {
    let dir = tempdir("none");
    for sub in ["work", "xdg", "home"] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    let candidate = repo_root().join("fixtures/images/candidate-64x48.png");
    let source = repo_root().join("fixtures/images/source-64x48.png");
    let run = |command: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_squint"))
            .current_dir(dir.join("work"))
            .args(command)
            .args(["--use-case", "source-fidelity", "--candidate"])
            .arg(&candidate)
            .arg("--source")
            .arg(&source)
            .env("XDG_CONFIG_HOME", dir.join("xdg"))
            .env("HOME", dir.join("home"))
            .output()
            .unwrap()
    };
    let estimated = run(&["estimate", "--json"]);
    assert_eq!(
        estimated.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&estimated.stderr)
    );
    let e: serde_json::Value = serde_json::from_slice(&estimated.stdout).unwrap();
    assert_eq!(e["refused"], "readiness");
    assert_eq!(e["worst_case_usd"], serde_json::Value::Null);
    assert_eq!(e["effective"]["model"], serde_json::Value::Null);
    assert_eq!(e["effective"]["config_identity"], serde_json::Value::Null);
    assert_eq!(e["readiness"]["requirements"][0]["state"], "missing");
    assert_eq!(std::fs::read_dir(dir.join("work")).unwrap().count(), 0);
    let o = run(&["check"]);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(64), "{stderr}");
    let expected = dir.join("xdg/squint/squint.toml");
    assert!(
        stderr.contains(expected.to_str().unwrap()) && stderr.contains("--config"),
        "{stderr}"
    );
    assert!(stderr.contains("./squint.toml"), "{stderr}");
    assert_eq!(std::fs::read_dir(dir.join("work")).unwrap().count(), 0);
}
