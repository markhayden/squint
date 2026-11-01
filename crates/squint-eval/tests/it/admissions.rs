//! Task 3.4a: the versioned admission record and its reader.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_eval::admissions::{Admission, Admissions, Stage};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_synthetic_records_read_and_only_confirmed_tools_are_admitted() {
    let a = Admissions::load(&root().join("fixtures/evalsets/admissions.toml")).unwrap();
    assert_eq!(a.admissions_version, 1);
    assert_eq!(a.admission.len(), 2);
    assert_eq!(a.admitted("source-fidelity"), ["classes"]);
    assert!(a.admitted("reality-checks").is_empty());
    assert_eq!(
        a.stage("regions", "source-fidelity"),
        Some(Stage::Provisional)
    );
    assert_eq!(
        a.stage("classes", "source-fidelity"),
        Some(Stage::Confirmed)
    );
    assert_eq!(a.stage("view", "source-fidelity"), None);
}

#[test]
fn records_round_trip_and_unknown_or_duplicate_records_are_refused() {
    let dir = std::env::temp_dir().join(format!("squint-admissions-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut a = Admissions::default();
    a.admission.push(Admission {
        tool: "align".into(),
        use_case: "source-fidelity".into(),
        stage: Stage::Confirmed,
        receipt: "rcpt-1".into(),
    });
    let path = dir.join("admissions.toml");
    a.write(&path).unwrap();
    assert_eq!(Admissions::load(&path).unwrap(), a);
    let text = std::fs::read_to_string(&path).unwrap();
    let err =
        Admissions::parse(&text.replace("stage = \"confirmed\"", "stage = \"maybe\"")).unwrap_err();
    assert!(err.to_string().contains("stage"), "{err}");
    let err = Admissions::parse(&text.replace("admissions_version = 1", "admissions_version = 2"))
        .unwrap_err();
    assert!(err.to_string().contains("admissions_version"), "{err}");
    let twice = text.clone() + &text[text.find("[[admission]]").unwrap()..];
    let err = Admissions::parse(&twice).unwrap_err();
    assert!(err.to_string().contains("twice"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}
