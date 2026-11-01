//! Task 3.1 CLI scenarios: `sets check`, `sets seal`, `sets verify-seal` and `sets fetch` on the
//! fixture set, with exit codes 0 (ok), 1 (the set or seal does not hold) and 64 (usage).
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn squint(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(args)
        .output()
        .unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).unwrap();
        }
    }
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-sets-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&repo_root().join("fixtures/evalsets"), &dir);
    dir
}

#[test]
fn sets_check_passes_on_the_fixture_tree_and_reports_the_counts() {
    let o = squint(&["sets", "check", "fixtures/evalsets"]);
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        out.contains("1 set") && out.contains("2 items") && out.contains("4 images"),
        "{out}"
    );
}

#[test]
fn sets_check_fails_with_exit_1_and_names_the_problem() {
    let s = scratch("broken");
    std::fs::remove_file(s.join("source-fidelity/pairs-v1/labels.toml")).unwrap();
    let o = squint(&["sets", "check", s.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("sets:") && err.contains("labels.toml"),
        "{err}"
    );
    // A root with no sets is a usage error.
    let empty = std::env::temp_dir().join(format!("squint-sets-cli-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&empty);
    std::fs::create_dir_all(&empty).unwrap();
    let o = squint(&["sets", "check", empty.to_str().unwrap()]);
    assert_eq!(
        o.status.code(),
        Some(64),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn sets_seal_then_verify_seal_by_path_or_by_id_under_a_root() {
    let s = scratch("seal");
    let set = s.join("source-fidelity/pairs-v1");
    // Sealing needs an attestation.
    let o = squint(&["sets", "seal", "--set", set.to_str().unwrap()]);
    assert_eq!(
        o.status.code(),
        Some(64),
        "a missing --attest is a usage error (exit 64 since task 7.5)"
    );
    let o = squint(&[
        "sets",
        "seal",
        "--set",
        set.to_str().unwrap(),
        "--attest",
        "no run has seen these items",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(set.join("seal.toml").exists());
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("sealed") && out.contains("source-fidelity/pairs-v1"),
        "{out}"
    );
    // Verify by id under the root.
    let o = squint(&[
        "sets",
        "verify-seal",
        "--root",
        s.to_str().unwrap(),
        "--set",
        "source-fidelity/pairs-v1",
    ]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    // Change a label; the seal no longer holds.
    let labels = set.join("labels.toml");
    let text = std::fs::read_to_string(&labels)
        .unwrap()
        .replace("expected = \"fail\"", "expected = \"pass\"");
    std::fs::write(&labels, text).unwrap();
    let o = squint(&["sets", "verify-seal", "--set", set.to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("labels"));
}

#[test]
fn sets_fetch_reports_what_it_placed() {
    let s = scratch("fetch");
    let set = s.join("source-fidelity/pairs-v1");
    let store = s.join("store/source-fidelity/pairs-v1/items/pair-01");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::rename(
        set.join("items/pair-01/candidate.png"),
        store.join("candidate.png"),
    )
    .unwrap();
    let manifest = set.join("set.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("images_repo = \".\"", "images_repo = \"../../store\"");
    std::fs::write(&manifest, text).unwrap();
    let o = squint(&["sets", "fetch", "--set", set.to_str().unwrap()]);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("1 fetched") && out.contains("3 present"),
        "{out}"
    );
}
