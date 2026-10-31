//! Task 7.7b: a use case's action over `squint serve` is one decision (table-manners 5, 10):
//! images by path inside the client's roots or inline, the result the decision's `run.json`,
//! `invalid` for a path outside the roots, and the session budget refusing before any request.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use squint_testkit::mcp_client::Client;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn out(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-serve-check-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn server(out: &Path, overrides: &[&str]) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_squint"));
    c.current_dir(repo_root())
        .args([
            "serve",
            "--config",
            "fixtures/config/minimal.toml",
            "--provider-script",
            "fixtures/scripts/held.json",
            "--out",
        ])
        .arg(out);
    for o in overrides {
        c.args(["--set", o]);
    }
    c
}

/// A client whose roots hold the fixture images.
fn client(out: &Path, overrides: &[&str]) -> Client {
    let mut client = Client::start(&mut server(out, overrides)).unwrap();
    client.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/images").display()
    )];
    client
}

fn image(name: &str) -> serde_json::Value {
    json!({ "path": repo_root().join("fixtures/images").join(name) })
}

fn inline(name: &str) -> serde_json::Value {
    let bytes = std::fs::read(repo_root().join("fixtures/images").join(name)).unwrap();
    json!({ "inline": { "media_type": "image/png", "data": squint_core::image::base64_encode(&bytes) } })
}

#[test]
fn every_use_case_is_a_tool() {
    let mut c = client(&out("tools"), &[]);
    let names: Vec<String> = c
        .list_tools()
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    for want in [
        "source_fidelity",
        "reference_consistency",
        "reality_checks",
        "brief_compliance",
        "product_fidelity",
    ] {
        assert!(names.contains(&want.to_string()), "{want}: {names:?}");
    }
}

#[test]
fn a_check_by_path_and_by_inline_bytes_reaches_the_same_verdict() {
    let mut c = client(&out("check"), &[]);
    let by_path = c
        .call(
            "source_fidelity",
            json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") }),
        )
        .unwrap();
    assert!(!by_path.is_error, "{by_path:?}");
    let r = by_path.structured.unwrap();
    assert_eq!(r["status"], "completed");
    assert_eq!(r["action"], "source_fidelity");
    let dir = PathBuf::from(r["run"]["dir"].as_str().unwrap());
    let on_disk: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(on_disk, r);
    let mut c = client(&out("check-inline"), &[]);
    let by_inline = c
        .call(
            "source_fidelity",
            json!({ "candidate": inline("candidate-64x48.png"), "source": inline("source-64x48.png") }),
        )
        .unwrap()
        .structured
        .unwrap();
    assert_eq!(by_inline["body"]["verdict"], r["body"]["verdict"]);
    assert_eq!(
        by_inline["images"]["in"][0]["image"]["file_sha256"],
        r["images"]["in"][0]["image"]["file_sha256"]
    );
}

#[test]
fn a_path_outside_the_clients_roots_is_invalid_input_and_spends_nothing() {
    let mut c = client(&out("roots"), &[]);
    c.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/jobs").display()
    )];
    let r = c
        .call(
            "source_fidelity",
            json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") }),
        )
        .unwrap();
    assert!(r.is_error);
    let r = r.structured.unwrap();
    assert_eq!(r["status"], "invalid");
    assert_eq!(r["error"]["code"], "input");
    assert_eq!(r["spend"]["usd"], 0.0);
}

/// `estimate` reads its images too, so it answers to the same roots as a check (review C1): a
/// path outside them is refused before it is opened, and a missing path is refused in the same
/// words, so nothing about the file (its digest, size or existence) comes back.
#[test]
fn estimate_refuses_a_path_outside_the_roots_without_reading_it() {
    // A catalog model, so an estimate that ignored the roots would go on to read the image.
    let mut c = client(&out("estimate-roots"), &["provider.model=claude-haiku-5-5"]);
    c.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/jobs").display()
    )];
    let expect = json!({ "file_sha256": "0".repeat(64), "width": 64, "height": 48,
                         "projection": "flat", "topology": "bounded" });
    let outside = repo_root().join("fixtures/images/candidate-64x48.png");
    let missing = repo_root().join("fixtures/images/no-such-image.png");
    let mut reasons = Vec::new();
    for path in [&outside, &missing] {
        let r = c
            .call(
                "estimate",
                json!({ "use_case": "source-fidelity",
                        "candidate": { "path": path, "expect": expect },
                        "source": image("source-64x48.png") }),
            )
            .unwrap();
        assert!(r.is_error, "{path:?}");
        let r = r.structured.unwrap();
        assert_eq!(r["status"], "invalid", "{r}");
        assert_eq!(r["error"]["code"], "input", "{r}");
        let reason = r["error"]["reason"].as_str().unwrap().to_string();
        assert!(!reason.contains("file_sha256"), "{reason}");
        reasons.push(reason.replace(&path.display().to_string(), "<path>"));
    }
    assert_eq!(
        reasons[0], reasons[1],
        "a missing file must read like one outside"
    );
}

#[test]
fn an_exhausted_session_budget_is_a_budget_error_before_any_request() {
    let mut c = client(&out("budget"), &["serve.session_budget_usd=0.000001"]);
    let r = c
        .call(
            "source_fidelity",
            json!({ "candidate": image("candidate-64x48.png"), "source": image("source-64x48.png") }),
        )
        .unwrap();
    assert!(r.is_error);
    let r = r.structured.unwrap();
    assert_eq!(r["status"], "error");
    assert_eq!(r["error"]["code"], "budget");
    assert_eq!(r["spend"]["usd"], 0.0);
}

#[test]
fn no_image_blocks_unless_asked_and_none_to_send() {
    let mut c = client(&out("blocks"), &[]);
    let r = c
        .call(
            "source_fidelity",
            json!({
                "candidate": image("candidate-64x48.png"),
                "source": image("source-64x48.png"),
                "image_blocks": true
            }),
        )
        .unwrap();
    assert!(r.images.is_empty(), "squint returns no images");
}
