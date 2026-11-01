//! Task 3.4a: `matrix.toml` parsing, cells, decisions and the frozen identity.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_eval::matrix::{Arm, Matrix};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn the_fixture_matrix_parses_into_cells_and_an_identity() {
    let m = Matrix::load(&root().join("fixtures/matrix/baseline-a.toml")).unwrap();
    assert_eq!(m.matrix.name, "baseline-a");
    assert_eq!(m.matrix.sets, ["source-fidelity/pairs-v1"]);
    assert_eq!(m.matrix.arms, [Arm::A]);
    let cells = m.cells();
    assert_eq!(cells.len(), 2, "two models times one arm");
    assert_eq!(cells[0].id(), "anthropic:claude-haiku-5-5/A");
    assert_eq!(cells[1].id(), "google-ai-studio:gemini-3.8-flash/A");
    assert_eq!(
        cells[1]
            .model
            .controls
            .get("thinking_level")
            .and_then(|v| v.as_str()),
        Some("low")
    );
    assert_eq!(m.decisions(2), 4, "items times cells times repetitions");
    let id = m.identity();
    assert_eq!(id.len(), 64);
    // The identity follows the content, not the file's formatting.
    let text = std::fs::read_to_string(root().join("fixtures/matrix/baseline-a.toml")).unwrap();
    let reformatted = text.replace("spend_cap_usd = 5.0", "spend_cap_usd   =  5.0");
    assert_eq!(Matrix::parse(&reformatted).unwrap().identity(), id);
    let changed = text.replace("spend_cap_usd = 5.0", "spend_cap_usd = 6.0");
    assert_ne!(Matrix::parse(&changed).unwrap().identity(), id);
}

#[test]
fn arms_parse_from_their_names_and_bad_matrices_are_refused() {
    assert_eq!(
        "D_regions".parse::<Arm>().unwrap(),
        Arm::DX("regions".into())
    );
    assert_eq!("D".parse::<Arm>().unwrap(), Arm::D);
    assert_eq!("C".parse::<Arm>().unwrap(), Arm::C);
    assert!("B".parse::<Arm>().is_err());
    let text = std::fs::read_to_string(root().join("fixtures/matrix/baseline-a.toml")).unwrap();
    for (from, to, message) in [
        ("arms = [\"A\"]", "arms = []", "arms"),
        ("decision_runs = 1", "decision_runs = 3", "decision_runs"),
        ("repetitions = 1", "repetitions = 0", "repetitions"),
        ("spend_cap_usd = 5.0", "spend_cap_usd = 0", "spend_cap_usd"),
        ("sets = [\"source-fidelity/pairs-v1\"]", "sets = []", "sets"),
        (
            "name = \"baseline-a\"",
            "name = \"baseline-a\"\nextra = 1",
            "extra",
        ),
    ] {
        let err = Matrix::parse(&text.replacen(from, to, 1))
            .unwrap_err()
            .to_string();
        assert!(err.contains(message), "{from} -> {to}: {err}");
    }
}

#[test]
fn a_model_may_carry_the_account_its_cells_consent_with() {
    let text = std::fs::read_to_string(root().join("fixtures/matrix/baseline-a.toml")).unwrap();
    let with = text.replace(
        "controls = { thinking_level = \"low\" }",
        "controls = { thinking_level = \"low\" }\naccount = \"google:${SQUINT_GOOGLE_ACCOUNT}\"",
    );
    let m = Matrix::parse(&with).unwrap();
    assert_eq!(m.matrix.models[0].account, None);
    assert_eq!(
        m.matrix.models[1].account.as_deref(),
        Some("google:${SQUINT_GOOGLE_ACCOUNT}"),
        "the reference, never the value"
    );
    assert_ne!(m.identity(), Matrix::parse(&text).unwrap().identity());
}

#[test]
fn the_shipped_baseline_matrix_parses_and_names_sets_that_exist() {
    let m = Matrix::load(&root().join("evalsets/matrix/baseline.toml")).unwrap();
    assert_eq!(m.matrix.arms, [Arm::A]);
    assert_eq!((m.matrix.decision_runs, m.matrix.repetitions), (1, 2));
    assert_eq!(m.matrix.sets.len(), 10);
    for id in &m.matrix.sets {
        assert!(
            root().join("evalsets").join(id).join("set.toml").is_file(),
            "{id}"
        );
    }
    assert!(root().join(&m.matrix.config).is_file());
    assert_eq!(m.matrix.models.len(), 6);
    for model in &m.matrix.models {
        if model.profile != "anthropic" {
            let a = model.account.as_deref().unwrap_or("");
            assert!(
                a.contains("${SQUINT_") && !a.contains('@'),
                "{a}: a reference, never a value"
            );
        }
    }
    assert_eq!(m.decisions(110), 1320);
}
