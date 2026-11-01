//! Task 1.11 CLI scenario: the same `check` twice shows `cache = "hit"` the second time.
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

fn run(out: &Path, cache_dir: &Path) -> (Option<i32>, String, serde_json::Value) {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/minimal.toml",
            "--set",
            &format!("cache.dir={}", cache_dir.display()),
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            "fixtures/scripts/held.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let dir = std::fs::read_dir(out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let r =
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stderr).to_string(),
        r,
    )
}

#[test]
fn the_second_identical_check_is_a_cache_hit() {
    let base = std::env::temp_dir().join(format!("squint-cache-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let cache_dir = base.join("cache");
    let (code, stderr, r) = run(&base.join("out-1"), &cache_dir);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(r["cache"]["status"], "miss");
    assert!(!stderr.contains("cache: hit"));
    let (code, stderr, r) = run(&base.join("out-2"), &cache_dir);
    assert_eq!(code, Some(0), "{stderr}");
    assert_eq!(r["cache"]["status"], "hit");
    assert!(stderr.contains("cache: hit (source: "), "{stderr}");
    assert_eq!(r["verdict"], "pass");
}
