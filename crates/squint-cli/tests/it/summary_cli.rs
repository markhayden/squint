//! Task 11.1: after `check`, stdout is still the verdict word alone and stderr still names the
//! decision; the human summary (items with reasons, findings, why, spend, the viewer command)
//! follows on stderr, and `--quiet` gives exactly the earlier output.
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

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-summary-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Exit code, stdout, stderr and the decision directory of one scripted check.
fn check(script: &str, out: &Path, extra: &[&str]) -> (Option<i32>, String, String, PathBuf) {
    let mut args = vec![
        "check",
        "--config",
        "fixtures/config/minimal.toml",
        "--job",
        "fixtures/jobs/pair.toml",
        "--provider-script",
        script,
        "--out",
        out.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(&args)
        .output()
        .unwrap();
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
        dirs.pop().unwrap(),
    )
}

#[test]
fn an_unsure_check_says_which_items_were_tentative_and_why() {
    let out = tempdir("unsure");
    let (code, stdout, stderr, dir) = check("fixtures/scripts/tentative-held-sf1.json", &out, &[]);
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(stdout, "unsure\n", "stdout is the verdict word alone");
    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines[0], format!("decision: {}", dir.display()), "{stderr}");
    assert_eq!(lines[1], "items:", "{stderr}");
    let sf1 = lines
        .iter()
        .find(|l| l.trim_start().starts_with("SF1@candidate"))
        .unwrap();
    assert!(
        sf1.contains("Every protected subject") && sf1.contains("held (tentative): scripted"),
        "{sf1}"
    );
    assert!(
        lines.contains(&"why: unsure: SF1@candidate held only tentatively"),
        "{stderr}"
    );
    let spent = lines.iter().find(|l| l.starts_with("spent: $")).unwrap();
    assert!(
        spent.contains("of the $2.00 cap") && spent.contains("worst-case envelope reserved $"),
        "{spent}"
    );
    assert_eq!(
        lines.last().unwrap(),
        &format!("view: squint report view {}", dir.display())
    );
}

#[test]
fn a_failing_check_names_the_violation_its_reason_and_where() {
    let out = tempdir("fail");
    let (code, stdout, stderr, _) = check("fixtures/scripts/violation.json", &out, &[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert_eq!(stdout, "fail\n");
    let sf2 = stderr
        .lines()
        .find(|l| l.trim_start().starts_with("SF2@candidate"))
        .unwrap();
    assert!(
        sf2.contains("violation (confident) at candidate (whole image): The camera moved"),
        "{sf2}"
    );
    assert!(
        stderr
            .lines()
            .any(|l| l == "why: fail: SF2@candidate violated (confident)"),
        "{stderr}"
    );
}

#[test]
fn findings_print_with_their_location() {
    let out = tempdir("finding");
    let (code, _, stderr, _) = check("fixtures/scripts/finding-violation.json", &out, &[]);
    assert_eq!(code, Some(1), "{stderr}");
    assert!(stderr.contains("findings:\n"), "{stderr}");
    assert!(
        stderr.contains("violation (confident) under SF1 at candidate box [0.10 0.10 0.30 0.30]"),
        "{stderr}"
    );
    assert!(
        stderr.contains("why: fail: a confident violation in a finding under SF1"),
        "{stderr}"
    );
}

#[test]
fn a_cache_hit_summarises_the_decision_it_copies() {
    let base = tempdir("cache");
    let cache = format!("cache.dir={}", base.join("cache").display());
    let (code, _, stderr, _) = check(
        "fixtures/scripts/held.json",
        &base.join("out-1"),
        &["--set", &cache],
    );
    assert_eq!(code, Some(0), "{stderr}");
    let (code, stdout, stderr, _) = check(
        "fixtures/scripts/held.json",
        &base.join("out-2"),
        &["--set", &cache],
    );
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(stdout, "pass\n");
    assert!(stderr.contains("cache: hit (source: "), "{stderr}");
    assert!(
        stderr.contains("why: pass: every mandatory item held"),
        "{stderr}"
    );
    assert!(stderr.contains("spent: $0 now (a cache hit"), "{stderr}");
}

#[test]
fn quiet_prints_only_the_verdict_and_the_decision() {
    let out = tempdir("quiet");
    let (code, stdout, stderr, dir) = check(
        "fixtures/scripts/tentative-held-sf1.json",
        &out,
        &["--quiet"],
    );
    assert_eq!(code, Some(2), "{stderr}");
    assert_eq!(stdout, "unsure\n");
    assert_eq!(stderr, format!("decision: {}\n", dir.display()));
}
