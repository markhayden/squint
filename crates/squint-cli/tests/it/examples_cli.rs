//! Task 2.10b: every shipped example config under `examples/` resolves for its use case, and the
//! `--set` overrides the live smoke uses to point one example at each direct profile pass the
//! catalog check. Dry runs only: nothing leaves the machine and no credential is read.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

const CANDIDATE: &str = "fixtures/images/candidate-64x48.png";
const SOURCE: &str = "fixtures/images/source-64x48.png";

/// The shorthand job each use case needs, and one rule id its base set must render.
fn job_for(use_case: &str) -> (Vec<&'static str>, &'static str) {
    match use_case {
        "source-fidelity" => (vec!["--source", SOURCE], "SF1@candidate"),
        "reference-consistency" => (
            vec!["--ref", "subject=fixtures/images/source-64x48.png"],
            "RC1@candidate",
        ),
        "reality-checks" => (vec![], "RE1@candidate"),
        "brief-compliance" => (vec!["--brief", "a red bicycle, no people"], "BC1@candidate"),
        "product-fidelity" => (
            vec!["--ref", "product=fixtures/images/source-64x48.png"],
            "PF1@candidate",
        ),
        other => panic!("unknown use case {other}"),
    }
}

fn dry_run(config: &str, use_case: &str, sets: &[&str]) -> std::process::Output {
    let (job, _) = job_for(use_case);
    let mut c = Command::new(env!("CARGO_BIN_EXE_squint"));
    c.current_dir(repo_root())
        .env("SQUINT_ANTHROPIC_ACCOUNT", "org-example")
        .env("SQUINT_GOOGLE_ACCOUNT", "project-example")
        .env("SQUINT_OPENAI_ACCOUNT", "org-example")
        .env_remove("ANTHROPIC_API_KEY")
        .env_remove("GEMINI_API_KEY")
        .env_remove("OPENAI_API_KEY")
        .args([
            "check",
            "--config",
            config,
            "--dry-run",
            "--use-case",
            use_case,
            "--candidate",
            CANDIDATE,
        ])
        .args(job);
    for s in sets {
        c.args(["--set", s]);
    }
    c.output().unwrap()
}

fn example_configs() -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = std::fs::read_dir(repo_root().join("examples"))
        .expect("examples/ exists")
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .map(|p| {
            let use_case = p.file_stem().unwrap().to_str().unwrap().to_string();
            (format!("examples/{use_case}.toml"), use_case)
        })
        .collect();
    found.sort();
    found
}

#[test]
fn one_example_config_per_use_case_resolves_in_a_dry_run() {
    let configs = example_configs();
    let use_cases: Vec<&str> = configs.iter().map(|(_, u)| u.as_str()).collect();
    assert_eq!(
        use_cases,
        [
            "brief-compliance",
            "product-fidelity",
            "reality-checks",
            "reference-consistency",
            "source-fidelity",
        ],
        "examples/ holds exactly one config per use case, named after it"
    );
    for (config, use_case) in &configs {
        let o = dry_run(config, use_case, &[]);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(0), "{config}: {err}");
        let out = String::from_utf8_lossy(&o.stdout);
        let (_, rule) = job_for(use_case);
        assert!(out.contains(rule), "{config} did not render {rule}:\n{out}");
    }
}

/// The vendors the live smoke runs, read from its `smoke <profile> <model> <key> <account> <prefix>`
/// lines so the test cannot drift from the script.
fn live_smoke_vendors() -> Vec<(String, String, String)> {
    let script = std::fs::read_to_string(repo_root().join("scripts/live-smoke.sh")).unwrap();
    let vendors: Vec<(String, String, String)> = script
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() == 6 && f[0] == "smoke").then(|| (f[1].into(), f[2].into(), f[5].into()))
        })
        .collect();
    assert_eq!(
        vendors.len(),
        3,
        "live-smoke.sh runs the three direct profiles"
    );
    vendors
}

#[test]
fn the_source_fidelity_example_points_at_each_live_smoke_vendor_with_set_overrides() {
    for (profile, model, prefix) in live_smoke_vendors() {
        let sets = [
            format!("provider.profile={profile}"),
            format!("provider.model={model}"),
            format!("privacy.account={prefix}:account-example"),
        ];
        let sets: Vec<&str> = sets.iter().map(String::as_str).collect();
        let o = dry_run("examples/source-fidelity.toml", "source-fidelity", &sets);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(0), "{sets:?}: {err}");
    }
}
