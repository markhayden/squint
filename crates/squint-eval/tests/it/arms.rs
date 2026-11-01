//! Task 3.4b: exact arm membership (SPEC "Runner and arms") from the admission record.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_eval::admissions::Admissions;
use squint_eval::arms::{ToolSelection, tools_for};
use squint_eval::matrix::Arm;

fn admissions() -> Admissions {
    Admissions::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/evalsets/admissions.toml"),
    )
    .unwrap()
}

fn sel(enabled: &[&str], projections: &[(&str, &str)]) -> ToolSelection {
    ToolSelection {
        enabled: enabled.iter().map(|s| s.to_string()).collect(),
        projections: projections
            .iter()
            .map(|(t, p)| (t.to_string(), p.to_string()))
            .collect(),
    }
}

#[test]
fn arm_membership_follows_the_spec_and_the_admission_record() {
    let a = admissions();
    assert_eq!(
        tools_for(&Arm::A, &a, "source-fidelity").unwrap(),
        sel(&[], &[])
    );
    assert_eq!(
        tools_for(&Arm::C, &a, "source-fidelity").unwrap(),
        sel(&["view", "align"], &[("align", "visual_only")]),
        "the visual-access control: view and align under visual_only"
    );
    assert_eq!(
        tools_for(&Arm::DX("regions".into()), &a, "source-fidelity").unwrap(),
        sel(&["view", "align", "regions"], &[("align", "visual_only")]),
        "C plus exactly the tool under test; align stays projected"
    );
    assert_eq!(
        tools_for(&Arm::DX("align".into()), &a, "source-fidelity").unwrap(),
        sel(&["view", "align"], &[]),
        "when align itself is under test it is not projected"
    );
    // D: C plus every confirmed tool for the use case; the provisional `regions` is excluded.
    assert_eq!(
        tools_for(&Arm::D, &a, "source-fidelity").unwrap(),
        sel(&["view", "align", "classes"], &[("align", "visual_only")])
    );
    assert_eq!(
        tools_for(&Arm::D, &a, "reality-checks").unwrap(),
        sel(&["view", "align"], &[("align", "visual_only")])
    );
    let err = tools_for(&Arm::DX("view".into()), &a, "source-fidelity").unwrap_err();
    assert!(err.to_string().contains("view"), "{err}");
}

#[test]
fn with_no_admission_record_arm_d_is_the_control() {
    // The evalsets root carries no admissions.toml until a gate admits something, and the runner
    // then reads an empty record: arm D is exactly arm C, view and align with align projected.
    let none = squint_eval::admissions::Admissions::default();
    for use_case in ["source-fidelity", "reality-checks"] {
        assert_eq!(
            tools_for(&Arm::D, &none, use_case).unwrap(),
            tools_for(&Arm::C, &none, use_case).unwrap(),
            "{use_case}"
        );
    }
    assert!(
        !std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../evalsets/admissions.toml")
            .exists(),
        "nothing is admitted yet: the regions gate was rejected (receipt 8e0c8307a72475ac)"
    );
}
