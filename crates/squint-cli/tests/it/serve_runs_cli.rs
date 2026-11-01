//! Task 8.5: runs by page (table-manners 4.3): `squint://runs` is the newest page and
//! `squint://runs?cursor=…` the next, newest first by (started, id), with an opaque cursor of
//! URI-unreserved characters; a run started during a walk is seen by reading page one again.
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
    let dir = std::env::temp_dir().join(format!("squint-serve-runs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn client(out: &Path, script: &str) -> Client {
    let mut c = Command::new(env!("CARGO_BIN_EXE_squint"));
    c.current_dir(repo_root())
        .args([
            "serve",
            "--config",
            "fixtures/config/minimal.toml",
            "--provider-script",
        ])
        .arg(repo_root().join("fixtures/scripts").join(script))
        .arg("--out")
        .arg(out);
    let mut client = Client::start(&mut c).unwrap();
    client.roots = vec![format!(
        "file://{}",
        repo_root().join("fixtures/images").display()
    )];
    client
}

/// A document valid against one of the contract's schemas, by id.
fn valid(id: &str, doc: &serde_json::Value) {
    struct Embedded;
    impl jsonschema::Retrieve for Embedded {
        fn retrieve(
            &self,
            uri: &jsonschema::Uri<String>,
        ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
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
    let schema: serde_json::Value = serde_json::from_str(text).unwrap();
    let validator = jsonschema::options()
        .with_retriever(Embedded)
        .build(&schema)
        .unwrap();
    let errors: Vec<String> = validator.iter_errors(doc).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{id}: {errors:?}\n{doc}");
}

fn page(c: &mut Client, cursor: Option<&str>) -> serde_json::Value {
    let uri = match cursor {
        Some(cursor) => format!("squint://runs?cursor={cursor}"),
        None => "squint://runs".to_string(),
    };
    let doc: serde_json::Value = serde_json::from_str(&c.read_resource(&uri).unwrap()).unwrap();
    valid("urn:table-manners:core:runs", &doc);
    doc
}

/// A run directory as a server leaves one while it is still going, started at `ms`.
fn plant(out: &Path, ms: u64) {
    std::fs::create_dir_all(out.join(format!("estimate-{ms}-1"))).unwrap();
}

#[test]
fn runs_are_walked_by_page_newest_first_each_once() {
    let dir = out("pages");
    for ms in 1_000..1_120 {
        plant(&dir, ms);
    }
    let mut c = client(&dir, "held.json");
    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let doc = page(&mut c, cursor.as_deref());
        pages += 1;
        for run in doc["runs"].as_array().unwrap() {
            ids.push(run["id"].as_str().unwrap().to_string());
        }
        match doc["next_cursor"].as_str() {
            Some(next) => {
                let unreserved = next
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || "-._~".contains(ch));
                assert!(unreserved, "{next}");
                cursor = Some(next.to_string());
            }
            None => break,
        }
        assert!(pages < 10, "the walk does not end");
    }
    assert!(pages > 1, "120 runs fit one page");
    let expected: Vec<String> = (1_000..1_120)
        .rev()
        .map(|ms| format!("estimate-{ms}-1"))
        .collect();
    assert_eq!(ids, expected);
}

#[test]
fn a_run_started_during_a_walk_is_seen_on_page_one() {
    let dir = out("during");
    for ms in 1_000..1_120 {
        plant(&dir, ms);
    }
    let mut c = client(&dir, "held.json");
    let first = page(&mut c, None);
    plant(&dir, 2_000);
    let second = page(&mut c, first["next_cursor"].as_str());
    assert!(
        second["runs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["id"] != "estimate-2000-1")
    );
    assert_eq!(page(&mut c, None)["runs"][0]["id"], "estimate-2000-1");
}

#[test]
fn a_malformed_cursor_is_no_resource() {
    let mut c = client(&out("bad-cursor"), "held.json");
    assert!(c.read_resource("squint://runs?cursor=%%%").is_err());
    assert!(
        c.read_resource("squint://runs?cursor=bm90LWEtcG9zaXRpb24")
            .is_err()
    );
}

#[test]
fn the_runs_template_is_listed() {
    let mut c = client(&out("template"), "held.json");
    let listed = c.request("resources/templates/list", json!({})).unwrap();
    assert!(
        listed["resourceTemplates"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["uriTemplate"] == "squint://runs{?cursor}"),
        "{listed}"
    );
}

/// Runs that share a start time are still walked each once when a page boundary falls among
/// them: the cursor carries the id as well as the time.
#[test]
fn runs_of_one_start_time_split_across_a_page() {
    let dir = out("same-ms");
    for n in 0..75 {
        std::fs::create_dir_all(dir.join(format!("estimate-5000-1-{n:02}"))).unwrap();
    }
    let mut c = client(&dir, "held.json");
    let first = page(&mut c, None);
    let second = page(&mut c, first["next_cursor"].as_str());
    assert!(second["next_cursor"].is_null(), "{second}");
    let mut ids: Vec<String> = [&first, &second]
        .iter()
        .flat_map(|doc| doc["runs"].as_array().unwrap().clone())
        .map(|run| run["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids.len(), 75);
    let walked = ids.clone();
    ids.sort();
    ids.dedup();
    ids.reverse();
    assert_eq!(walked, ids, "newest first by (started, id), none twice");
}
