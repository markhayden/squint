//! Task 3.4a: the execution descriptor over every answer-affecting input, the resume check and
//! the single-writer lock.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_eval::descriptor::{Descriptor, Inputs, WriterLock};
use squint_eval::matrix::Matrix;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn inputs(matrix_text: &str) -> Inputs {
    Inputs {
        matrix: Matrix::parse(matrix_text).unwrap(),
        matrix_text: matrix_text.to_string(),
        config_text: "[provider]\nprofile = \"fake\"\n".into(),
        evalsets_root: root().join("fixtures/evalsets"),
        tools: vec![(
            "fake_regions".into(),
            "{\"name\":\"fake_regions\"}".into(),
            Some("lock-1".into()),
        )],
        selections: [(
            "C@source-fidelity".to_string(),
            vec!["view".to_string(), "align:visual_only".to_string()],
        )]
        .into_iter()
        .collect(),
        harness_version: "0.1.0 (test)".into(),
    }
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-descriptor-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_descriptor_covers_every_input_and_refuses_resume_on_any_change() {
    let text = std::fs::read_to_string(root().join("fixtures/matrix/baseline-a.toml")).unwrap();
    let d = Descriptor::compute(&inputs(&text)).unwrap();
    assert_eq!(d.descriptor_version, 1);
    assert_eq!(d.matrix_identity, Matrix::parse(&text).unwrap().identity());
    assert_eq!(d.prompt_version, squint_core::render::PROMPT_VERSION);
    assert_eq!(
        d.catalog_version,
        squint_providers::catalog::CATALOG_VERSION
    );
    assert_eq!(d.sets.len(), 1);
    let set = &d.sets["source-fidelity/pairs-v1"];
    assert_eq!(set.images.len(), 64);
    assert_eq!(set.rules.len(), 64);
    assert_eq!(set.labels.len(), 64);
    assert_eq!(
        d.tools["fake_regions"].lock_digest.as_deref(),
        Some("lock-1")
    );
    assert_eq!(d.sha256.len(), 64);
    let dir = tempdir("resume");
    d.write(&dir).unwrap();
    assert_eq!(Descriptor::read(&dir).unwrap(), d);
    Descriptor::check_resume(&dir, &d).unwrap();
    // Each input changed in turn refuses resume and names what changed.
    let mut other = inputs(&text.replace("spend_cap_usd = 5.0", "spend_cap_usd = 9.0"));
    let err = Descriptor::check_resume(&dir, &Descriptor::compute(&other).unwrap()).unwrap_err();
    assert!(err.to_string().contains("matrix"), "{err}");
    other = inputs(&text);
    other.harness_version = "0.2.0".into();
    let err = Descriptor::check_resume(&dir, &Descriptor::compute(&other).unwrap()).unwrap_err();
    assert!(err.to_string().contains("harness"), "{err}");
    other = inputs(&text);
    other.tools[0].2 = Some("lock-2".into());
    let err = Descriptor::check_resume(&dir, &Descriptor::compute(&other).unwrap()).unwrap_err();
    assert!(err.to_string().contains("tool"), "{err}");
    other = inputs(&text);
    other.config_text.push_str("[budget]\nretries = 1\n");
    let err = Descriptor::check_resume(&dir, &Descriptor::compute(&other).unwrap()).unwrap_err();
    assert!(err.to_string().contains("config"), "{err}");
    // A set that changed on disk: mutate a copy of the evalsets root.
    let copy = tempdir("evalsets-copy");
    copy_dir(&root().join("fixtures/evalsets"), &copy);
    let labels = copy.join("source-fidelity/pairs-v1/labels.toml");
    let t = std::fs::read_to_string(&labels)
        .unwrap()
        .replace("expected = \"fail\"", "expected = \"pass\"");
    std::fs::write(&labels, t).unwrap();
    other = inputs(&text);
    other.evalsets_root = copy;
    let err = Descriptor::check_resume(&dir, &Descriptor::compute(&other).unwrap()).unwrap_err();
    assert!(
        err.to_string().contains("pairs-v1") && err.to_string().contains("labels"),
        "{err}"
    );
    // A missing descriptor is not a resume at all.
    let empty = tempdir("empty");
    assert!(Descriptor::read(&empty).is_err());
}

#[test]
fn one_writer_at_a_time() {
    let dir = tempdir("lock");
    let first = WriterLock::acquire(&dir).unwrap();
    let err = WriterLock::acquire(&dir).unwrap_err();
    assert!(err.to_string().contains("another writer"), "{err}");
    drop(first);
    let second = WriterLock::acquire(&dir).unwrap();
    drop(second);
    // A lock left by a process that no longer exists is stale and can be taken.
    std::fs::write(dir.join("LOCK"), "pid = 999999999\n").unwrap();
    WriterLock::acquire(&dir).unwrap();
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
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

#[test]
fn a_changed_arm_selection_refuses_resume() {
    let dir = tempdir("selections");
    let text = std::fs::read_to_string(root().join("fixtures/matrix/baseline-a.toml")).unwrap();
    let first = Descriptor::compute(&inputs(&text)).unwrap();
    first.write(&dir).unwrap();
    let mut changed = inputs(&text);
    changed.selections.insert(
        "D@source-fidelity".into(),
        vec!["view".into(), "align:visual_only".into(), "classes".into()],
    );
    let now = Descriptor::compute(&changed).unwrap();
    assert_ne!(now.sha256, first.sha256);
    let err = Descriptor::check_resume(&dir, &now)
        .unwrap_err()
        .to_string();
    assert!(err.contains("arm selections"), "{err}");
}
