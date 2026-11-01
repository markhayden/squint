//! Task 3.1: set manifests, `check`, `seal` and `verify-seal` (SPEC "Set manifest", "Seal").
//! The fixture set under fixtures/evalsets is valid; every failure mode is a mutation of a copy.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use squint_eval::sets::{self, Split};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture() -> PathBuf {
    root().join("fixtures/evalsets/source-fidelity/pairs-v1")
}

/// A scratch copy of the evalsets root holding the fixture set, for mutation.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-sets-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&root().join("fixtures/evalsets"), &dir);
    dir
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

fn set_dir(scratch: &Path) -> PathBuf {
    scratch.join("source-fidelity/pairs-v1")
}

fn rewrite(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains(from), "{}: {from} not found", path.display());
    std::fs::write(path, text.replacen(from, to, 1)).unwrap();
}

fn check_err(root: &Path) -> String {
    match sets::check(root) {
        Ok(report) => panic!("check passed: {report:?}"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn the_fixture_set_parses_and_checks() {
    let set = sets::Set::load(&fixture()).unwrap();
    assert_eq!(set.manifest.set.id, "source-fidelity/pairs-v1");
    assert_eq!(set.manifest.set.split, Split::Dev);
    assert_eq!(set.manifest.item.len(), 2);
    let report = sets::check(&root().join("fixtures/evalsets")).unwrap();
    assert_eq!(report.sets, 1);
    assert_eq!(report.items, 2);
    assert_eq!(report.images, 4);
}

#[test]
fn check_fails_on_an_image_whose_hash_changed() {
    let s = scratch("hash");
    std::fs::copy(
        root().join("fixtures/images/two-to-one-128x64.png"),
        set_dir(&s).join("items/pair-01/candidate.png"),
    )
    .unwrap();
    let err = check_err(&s);
    assert!(err.contains("pair-01") && err.contains("sha256"), "{err}");
}

#[test]
fn check_fails_on_a_missing_image() {
    let s = scratch("missing");
    std::fs::remove_file(set_dir(&s).join("items/pair-02/source.png")).unwrap();
    let err = check_err(&s);
    assert!(
        err.contains("pair-02") && err.contains("source.png"),
        "{err}"
    );
}

#[test]
fn check_fails_when_a_job_does_not_resolve_against_the_pinned_rules() {
    // Source fidelity requires a source image; drop it from one job.
    let s = scratch("roles");
    let job = set_dir(&s).join("items/pair-01/job.toml");
    let text = std::fs::read_to_string(&job).unwrap();
    let cut = text.find("[[job.images]]\nid = \"source\"").unwrap();
    std::fs::write(&job, &text[..cut]).unwrap();
    let err = check_err(&s);
    assert!(
        err.contains("pair-01") && err.to_lowercase().contains("source"),
        "{err}"
    );
}

#[test]
fn check_fails_when_the_job_use_case_differs_from_the_set() {
    let s = scratch("use-case");
    rewrite(
        &set_dir(&s).join("items/pair-01/job.toml"),
        "use_case = \"source-fidelity\"",
        "use_case = \"reality-checks\"",
    );
    let err = check_err(&s);
    assert!(err.contains("pair-01") && err.contains("use case"), "{err}");
}

#[test]
fn check_fails_on_an_undeclared_family_and_on_a_family_in_two_splits() {
    let s = scratch("family");
    rewrite(
        &set_dir(&s).join("set.toml"),
        "family = \"rotated\"",
        "family = \"unknown\"",
    );
    let err = check_err(&s);
    assert!(err.contains("unknown") && err.contains("famil"), "{err}");

    // A second set of the same use case, heldout split, reusing the family "gradient".
    let s = scratch("split");
    let other = s.join("source-fidelity/pairs-heldout");
    copy_dir(&set_dir(&s), &other);
    rewrite(
        &other.join("set.toml"),
        "id = \"source-fidelity/pairs-v1\"",
        "id = \"source-fidelity/pairs-heldout\"",
    );
    rewrite(
        &other.join("set.toml"),
        "split = \"dev\"",
        "split = \"heldout\"",
    );
    let err = check_err(&s);
    assert!(
        err.contains("gradient") && err.contains("dev") && err.contains("heldout"),
        "{err}"
    );
}

#[test]
fn check_fails_on_label_problems_per_split() {
    // A dev item without a label.
    let s = scratch("unlabelled");
    let labels = set_dir(&s).join("labels.toml");
    let text = std::fs::read_to_string(&labels).unwrap();
    let cut = text.find("[[label]]\nitem = \"pair-02\"").unwrap();
    std::fs::write(&labels, &text[..cut]).unwrap();
    let err = check_err(&s);
    assert!(err.contains("pair-02") && err.contains("label"), "{err}");

    // A label for an item the set does not have.
    let s = scratch("stray-label");
    rewrite(
        &set_dir(&s).join("labels.toml"),
        "item = \"pair-02\"",
        "item = \"pair-09\"",
    );
    let err = check_err(&s);
    assert!(err.contains("pair-09"), "{err}");

    // A dev set with no labels file at all.
    let s = scratch("no-labels");
    std::fs::remove_file(set_dir(&s).join("labels.toml")).unwrap();
    let err = check_err(&s);
    assert!(err.contains("labels.toml"), "{err}");

    // A blind set must carry labels_sha256 and no labels file.
    let s = scratch("blind");
    rewrite(
        &set_dir(&s).join("set.toml"),
        "split = \"dev\"",
        "split = \"blind\"",
    );
    let err = check_err(&s);
    assert!(err.contains("blind") && err.contains("labels"), "{err}");
}

#[test]
fn check_fails_on_a_rules_version_mismatch() {
    let s = scratch("rules-version");
    rewrite(&set_dir(&s).join("set.toml"), "version = 1", "version = 7");
    let err = check_err(&s);
    assert!(err.contains("version") && err.contains("7"), "{err}");
}

#[test]
fn a_public_manifest_or_job_cannot_carry_an_answer_label() {
    let s = scratch("leak");
    rewrite(
        &set_dir(&s).join("set.toml"),
        "family = \"gradient\"",
        "family = \"gradient\"\nexpected = \"pass\"",
    );
    let err = check_err(&s);
    assert!(err.contains("expected"), "{err}");
    let s = scratch("leak-job");
    rewrite(
        &set_dir(&s).join("items/pair-01/job.toml"),
        "brief = \"Restyle the scene; keep the structure.\"",
        "brief = \"Restyle the scene; keep the structure.\"\nexpected = \"pass\"",
    );
    let err = check_err(&s);
    assert!(err.contains("expected"), "{err}");
}

// ---- seal ----------------------------------------------------------------------------------

fn sealed(tag: &str) -> PathBuf {
    let s = scratch(tag);
    sets::seal(&set_dir(&s), "no judge or tool has seen these items", None).unwrap();
    s
}

fn verify_err(s: &Path) -> String {
    match sets::verify_seal(&set_dir(s)) {
        Ok(()) => panic!("the seal still verified"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn seal_writes_every_component_and_verifies_until_something_changes() {
    let s = sealed("seal");
    let seal = sets::Seal::load(&set_dir(&s)).unwrap();
    assert_eq!(seal.seal.sha256.len(), 64);
    assert_eq!(
        seal.seal.attestation,
        "no judge or tool has seen these items"
    );
    assert!(seal.seal.created.ends_with('Z'), "{}", seal.seal.created);
    let c = &seal.seal.components;
    for h in [
        &c.items,
        &c.families,
        &c.jobs,
        &c.images,
        &c.rules,
        &c.labels,
    ] {
        assert_eq!(h.len(), 64);
    }
    sets::verify_seal(&set_dir(&s)).unwrap();
    // Sealing twice with the same content gives the same hash (the timestamp is outside it).
    let again = sets::seal(&set_dir(&s), "second attestation", None).unwrap();
    assert_eq!(again.seal.sha256, seal.seal.sha256);
}

#[test]
fn verify_seal_names_the_component_that_changed() {
    // Membership: an item removed from the manifest (and its label, so check still passes).
    let s = sealed("mut-items");
    let manifest = set_dir(&s).join("set.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    let cut = text.find("[[item]]\nid = \"pair-02\"").unwrap();
    std::fs::write(&manifest, &text[..cut]).unwrap();
    let labels = set_dir(&s).join("labels.toml");
    let text = std::fs::read_to_string(&labels).unwrap();
    let cut = text.find("[[label]]\nitem = \"pair-02\"").unwrap();
    std::fs::write(&labels, &text[..cut]).unwrap();
    assert!(verify_err(&s).contains("items"), "{}", verify_err(&s));

    // A family assignment.
    let s = sealed("mut-family");
    rewrite(
        &set_dir(&s).join("set.toml"),
        "family = \"rotated\"",
        "family = \"gradient\"",
    );
    assert!(verify_err(&s).contains("families"), "{}", verify_err(&s));

    // A job field.
    let s = sealed("mut-job");
    rewrite(
        &set_dir(&s).join("items/pair-01/job.toml"),
        "keep the structure",
        "keep the colours",
    );
    assert!(verify_err(&s).contains("jobs"), "{}", verify_err(&s));

    // An image (re-pinned so the job itself is consistent; the seal still remembers the old one).
    let s = sealed("mut-image");
    let other = std::fs::read(root().join("fixtures/images/two-to-one-128x64.png")).unwrap();
    std::fs::write(set_dir(&s).join("items/pair-01/candidate.png"), &other).unwrap();
    let job = set_dir(&s).join("items/pair-01/job.toml");
    let text = std::fs::read_to_string(&job).unwrap();
    let old = text.split("sha256 = \"").nth(1).unwrap()[..64].to_string();
    rewrite(&job, &old, &squint_core::image::sha256_hex(&other));
    let err = verify_err(&s);
    assert!(err.contains("images") || err.contains("jobs"), "{err}");

    // The rules pin.
    let s = sealed("mut-rules");
    rewrite(&set_dir(&s).join("set.toml"), "version = 1", "version = 2");
    assert!(verify_err(&s).contains("rules"), "{}", verify_err(&s));

    // The labels.
    let s = sealed("mut-labels");
    rewrite(
        &set_dir(&s).join("labels.toml"),
        "expected = \"fail\"",
        "expected = \"pass\"",
    );
    assert!(verify_err(&s).contains("labels"), "{}", verify_err(&s));
}

#[test]
fn sealing_a_blind_set_moves_the_labels_out_and_pins_their_hash() {
    let s = scratch("blind-seal");
    rewrite(
        &set_dir(&s).join("set.toml"),
        "split = \"dev\"",
        "split = \"blind\"",
    );
    let outside = s.join("owner-storage");
    std::fs::create_dir_all(&outside).unwrap();
    let labels_bytes = std::fs::read(set_dir(&s).join("labels.toml")).unwrap();
    let seal = sets::seal(&set_dir(&s), "labelled before any run", Some(&outside)).unwrap();
    assert!(
        !set_dir(&s).join("labels.toml").exists(),
        "labels moved out"
    );
    assert!(outside.join("labels.toml").exists());
    let set = sets::Set::load(&set_dir(&s)).unwrap();
    assert_eq!(set.manifest.set.labels, None);
    assert_eq!(
        set.manifest.set.labels_sha256.as_deref(),
        Some(squint_core::image::sha256_hex(&labels_bytes).as_str())
    );
    assert_eq!(
        seal.seal.components.labels,
        squint_core::image::sha256_hex(&labels_bytes)
    );
    // The blind set now checks and verifies without its labels.
    sets::check(&s).unwrap();
    sets::verify_seal(&set_dir(&s)).unwrap();
    // A blind set with its labels still inside cannot be sealed without moving them out.
    let s2 = scratch("blind-inside");
    rewrite(
        &set_dir(&s2).join("set.toml"),
        "split = \"dev\"",
        "split = \"blind\"",
    );
    let err = sets::seal(&set_dir(&s2), "x", None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("blind") && err.contains("labels"), "{err}");
}

// ---- fetch ---------------------------------------------------------------------------------

/// Moves the set's images into a store laid out as `<store>/<set id>/<image path>` and points
/// the manifest at it; returns the store.
fn move_images_to_store(s: &Path, repo_value: &str) -> PathBuf {
    let store = s.join("store");
    for item in ["pair-01", "pair-02"] {
        for name in ["candidate.png", "source.png"] {
            let rel = format!("items/{item}/{name}");
            let to = store.join("source-fidelity/pairs-v1").join(&rel);
            std::fs::create_dir_all(to.parent().unwrap()).unwrap();
            std::fs::rename(set_dir(s).join(&rel), to).unwrap();
        }
    }
    rewrite(
        &set_dir(s).join("set.toml"),
        "images_repo = \".\"",
        &format!("images_repo = \"{repo_value}\""),
    );
    store
}

#[tokio::test]
async fn fetch_restores_missing_images_from_a_directory_store_and_verifies_them() {
    let s = scratch("fetch-dir");
    move_images_to_store(&s, "../../store");
    assert!(check_err(&s).contains("cannot be read"));
    let report = sets::fetch(&set_dir(&s)).await.unwrap();
    assert_eq!((report.fetched, report.present), (4, 0));
    sets::check(&s).unwrap();
    // A second fetch finds everything in place.
    let report = sets::fetch(&set_dir(&s)).await.unwrap();
    assert_eq!((report.fetched, report.present), (0, 4));
}

#[tokio::test]
async fn fetch_refuses_an_image_whose_hash_does_not_match_and_leaves_nothing_behind() {
    let s = scratch("fetch-mismatch");
    let store = move_images_to_store(&s, "../../store");
    std::fs::copy(
        root().join("fixtures/images/two-to-one-128x64.png"),
        store.join("source-fidelity/pairs-v1/items/pair-02/candidate.png"),
    )
    .unwrap();
    let err = sets::fetch(&set_dir(&s)).await.unwrap_err().to_string();
    assert!(
        err.contains("pair-02") && err.contains("candidate") && err.contains("sha256"),
        "{err}"
    );
    assert!(!set_dir(&s).join("items/pair-02/candidate.png").exists());
    assert!(
        !set_dir(&s)
            .join("items/pair-02/candidate.png.part")
            .exists()
    );
}

#[tokio::test]
async fn fetch_downloads_over_http_by_set_id_and_path() {
    use std::io::{Read, Write};
    let s = scratch("fetch-http");
    let store = move_images_to_store(&s, "placeholder");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    rewrite(
        &set_dir(&s).join("set.toml"),
        "images_repo = \"placeholder\"",
        &format!("images_repo = \"http://127.0.0.1:{port}\""),
    );
    std::thread::spawn(move || {
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = vec![0u8; 8192];
            let mut read = 0;
            loop {
                let n = stream.read(&mut buf[read..]).unwrap_or(0);
                if n == 0 || buf[..read + n].windows(4).any(|w| w == b"\r\n\r\n") {
                    read += n;
                    break;
                }
                read += n;
            }
            let head = String::from_utf8_lossy(&buf[..read]).to_string();
            let path = head
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .trim_start_matches('/')
                .to_string();
            let body = std::fs::read(store.join(&path)).unwrap_or_default();
            let status = if body.is_empty() {
                "404 Not Found"
            } else {
                "200 OK"
            };
            let _ = stream.write_all(
                format!("HTTP/1.1 {status}\r\nContent-Type: image/png\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
                    .as_bytes(),
            );
            let _ = stream.write_all(&body);
        }
    });
    let report = sets::fetch(&set_dir(&s)).await.unwrap();
    assert_eq!(report.fetched, 4);
    sets::check(&s).unwrap();
}

#[test]
fn github_image_urls_name_the_pinned_revision() {
    let url = sets::image_url(
        "github.com/markhayden/squint-evalsets",
        Some("0123abcd"),
        "reference-consistency/pets-v1",
        Path::new("items/pip-bridge-02/candidate.png"),
    )
    .unwrap();
    assert_eq!(
        url,
        "https://raw.githubusercontent.com/markhayden/squint-evalsets/0123abcd/reference-consistency/pets-v1/items/pip-bridge-02/candidate.png"
    );
    let err = sets::image_url(
        "github.com/markhayden/squint-evalsets",
        None,
        "x/y",
        Path::new("a.png"),
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("images_rev"), "{err}");
    assert_eq!(
        sets::image_url(
            "https://example.invalid/sets",
            None,
            "x/y",
            Path::new("a.png")
        )
        .unwrap(),
        "https://example.invalid/sets/x/y/a.png"
    );
    assert!(
        sets::image_url("../store", None, "x/y", Path::new("a.png")).is_err(),
        "a directory is not a URL"
    );
}

#[test]
fn the_published_sets_are_consistent_at_the_manifest_level() {
    // Offline: the images live in the other repository, so only manifests, families and labels
    // are read here. Every published set is labelled in full, sized as its split demands, and no
    // family of a use case appears in two splits; the README lists each one. A sealed blind set
    // carries only its labels' hash, the labels being in owner storage until its gate.
    use squint_eval::sets::{Families, Labels, Manifest, Split};
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evalsets");
    squint_eval::sets::check_families(&root).unwrap();
    let readme = std::fs::read_to_string(root.join("README.md")).unwrap();
    let dirs = squint_eval::sets::discover(&root).unwrap();
    assert_eq!(dirs.len(), 12, "{dirs:?}");
    for dir in dirs {
        let manifest: Manifest =
            toml::from_str(&std::fs::read_to_string(dir.join("set.toml")).unwrap()).unwrap();
        let families: Families =
            toml::from_str(&std::fs::read_to_string(dir.join(&manifest.set.families)).unwrap())
                .unwrap();
        let id = &manifest.set.id;
        assert_eq!(root.join(id), dir, "{id} lives at its id");
        let adversarial = id.ends_with("/adversarial-v1");
        // The five adversarial sets share one README row.
        let row = if adversarial {
            "`*/adversarial-v1`".to_string()
        } else {
            format!("`{id}`")
        };
        assert!(readme.contains(&row), "README.md lists {id}");
        // A sealed blind set carries only its labels' hash until its gate publishes them.
        if manifest.set.split == Split::Blind {
            assert!(
                manifest.set.labels.is_none(),
                "{id}: no labels beside a blind set"
            );
            assert!(manifest.set.labels_sha256.is_some(), "{id}: labels hash");
            assert!(
                manifest.item.len() >= 20,
                "{id}: an initial set holds at least 20"
            );
            continue;
        }
        // The two gated blind sets became heldout when their gates published the labels
        // (receipts 8e0c8307a72475ac and be41ae402334a394); every other published set is a dev set.
        let gated = ["source-fidelity/blind-v1", "source-fidelity/blind-v2"];
        assert_eq!(
            manifest.item.len(),
            match id.as_str() {
                _ if adversarial => 2,
                "source-fidelity/blind-v2" => 25,
                _ => 20,
            },
            "{id}"
        );
        let expected_split = if gated.contains(&id.as_str()) {
            Split::Heldout
        } else {
            Split::Dev
        };
        assert_eq!(manifest.set.split, expected_split, "{id}");
        let labels: Labels = toml::from_str(
            &std::fs::read_to_string(dir.join(manifest.set.labels.as_ref().unwrap())).unwrap(),
        )
        .unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for l in &labels.label {
            assert!(
                manifest.item.iter().any(|i| i.id == l.item),
                "{id}: label for unknown item {}",
                l.item
            );
            assert!(
                seen.insert(l.item.clone()),
                "{id}: {} labelled twice",
                l.item
            );
            assert!(
                !l.words.is_empty() && !l.adjudicated_by.is_empty(),
                "{id}: {} is unadjudicated",
                l.item
            );
        }
        assert_eq!(seen.len(), manifest.item.len(), "{id}: every item labelled");
        for item in &manifest.item {
            assert!(
                families.family.iter().any(|f| f.id == item.family),
                "{id}: {} names an undeclared family",
                item.id
            );
        }
        let failure_types: std::collections::BTreeSet<&str> = labels
            .label
            .iter()
            .flat_map(|l| l.failure_types.iter().map(String::as_str))
            .collect();
        if !adversarial {
            assert!(
                failure_types.len() >= 2,
                "{id}: failure types {failure_types:?}"
            );
            let fails = labels
                .label
                .iter()
                .filter(|l| l.expected == squint_eval::sets::Expected::Fail)
                .count();
            assert!(
                (5..=15).contains(&fails),
                "{id}: {fails} expected fails of {}",
                manifest.item.len()
            );
        }
    }
}

#[test]
fn every_published_seal_holds_and_every_receipt_agrees_with_its_set() {
    // The gate's publication (task 5.6) leaves a set, its seal, the published labels, a receipt
    // under evalsets/receipts and a copy of the receipt beside the results document. They must
    // tell one story: the seal verifies, the receipt's seal and label hashes are the set's, a
    // set that was blind carries the owner's attestation, and the two receipt copies are equal.
    use squint_eval::gate::Receipt;
    use squint_eval::sets::Seal;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evalsets");
    // Verifying a seal re-hashes the images, which live in the evalsets repository and are fetched
    // on demand (CI never fetches them); the seal is verified where they are present, and the
    // receipt checks below need only the manifests, the seal files and the labels.
    for dir in squint_eval::sets::discover(&root).unwrap() {
        if has_images(&dir) {
            squint_eval::sets::verify_seal(&dir)
                .unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
        }
    }
    let receipts = Receipt::load_all(&root).unwrap();
    assert!(!receipts.is_empty(), "the regions gate left a receipt");
    for (id, receipt) in &receipts {
        let r = &receipt.receipt;
        let set = root.join(&r.set);
        assert!(set.join("set.toml").is_file(), "{id}: {} exists", r.set);
        let seal: Seal =
            toml::from_str(&std::fs::read_to_string(set.join("seal.toml")).unwrap()).unwrap();
        assert_eq!(
            seal.seal.sha256, r.seal_sha256,
            "{id}: the seal is the set's"
        );
        assert!(
            !seal.seal.attestation.trim().is_empty(),
            "{id}: the owner's attestation is recorded in the seal"
        );
        let labels = std::fs::read(set.join("labels.toml")).unwrap();
        let labels_sha256: String = <sha2::Sha256 as sha2::Digest>::digest(&labels)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            labels_sha256, r.labels_sha256,
            "{id}: the published labels are the scored ones"
        );
        assert_eq!(
            seal.seal.components.labels, r.labels_sha256,
            "{id}: and the seal's labels component"
        );
        let copies: Vec<PathBuf> = glob_receipts(&root.join("../docs/results"), id);
        assert!(
            !copies.is_empty(),
            "{id}: a copy sits beside a results document"
        );
        let original = std::fs::read(root.join("receipts").join(format!("{id}.toml"))).unwrap();
        for copy in copies {
            assert_eq!(
                std::fs::read(&copy).unwrap(),
                original,
                "{}",
                copy.display()
            );
        }
    }
}

/// Whether any image of a set has been fetched into its directory.
fn has_images(dir: &Path) -> bool {
    fn walk(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                let p = e.path();
                if p.is_dir() {
                    walk(&p)
                } else {
                    p.extension().is_some_and(|x| x == "png" || x == "jpg")
                }
            })
    }
    walk(dir)
}

/// Every `receipt.toml` under the results documents whose id matches.
fn glob_receipts(results: &Path, id: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(results).unwrap().flatten() {
        let candidate = entry.path().join("receipt.toml");
        if candidate.is_file()
            && std::fs::read_to_string(&candidate)
                .unwrap()
                .contains(&format!("id = \"{id}\""))
        {
            out.push(candidate);
        }
    }
    out
}
