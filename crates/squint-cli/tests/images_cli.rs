//! Task 1.2 CLI scenarios: an EXIF-rotated candidate reports upright dimensions; an oversized
//! image is refused with `error: resolve` naming the bound. Requires the `testkit` feature.
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

fn run_check(job: &str, out: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/minimal.toml",
            "--job",
            &format!("fixtures/jobs/{job}"),
            "--provider-script",
            "fixtures/scripts/held.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

fn report(out: &Path) -> serde_json::Value {
    let run_dir = std::fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    serde_json::from_str(&std::fs::read_to_string(run_dir.join("report.json")).unwrap()).unwrap()
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-images-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn exif_rotated_candidate_reports_upright_dimensions_and_both_hashes() {
    let out = tempdir("exif");
    let o = run_check("exif-rotated.toml", &out);
    assert_eq!(
        o.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let r = report(&out);
    let candidate = r["runs"][0]["images"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "candidate")
        .unwrap();
    assert_eq!(candidate["width"], 48);
    assert_eq!(candidate["height"], 64);
    assert_eq!(candidate["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(candidate["canonical_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(candidate["projection"], "flat");
}

#[test]
fn oversized_candidate_is_refused_as_a_resolve_error_naming_the_bound() {
    let out = tempdir("oversized");
    let o = run_check("oversized.toml", &out);
    // An invalid request exits 64 whatever the policy (task 7.4).
    assert_eq!(
        o.status.code(),
        Some(64),
        "stderr: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(stderr.contains("50000000"), "names the bound: {stderr}");
    let r = report(&out);
    assert_eq!(r["runs"][0]["error"]["class"], "resolve");
    assert!(r["verdict"].is_null());
}
