//! Task 3.4c: each estimated decision envelope equals the ledger's reservation for that decision
//! on a fake run; totals are their sum; a matrix whose sum exceeds the cap can still complete.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use squint_core::config::Config;
use squint_core::provider::Provider;
use squint_core::tool::ToolRegistry;
use squint_eval::arms::ToolSelection;
use squint_eval::estimate::estimate;
use squint_eval::matrix::{Cell, Matrix};
use squint_eval::runner::{Factories, Plan, run};
use squint_testkit::ScriptedProvider;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Fake;

impl Factories for Fake {
    fn provider(&self, _cell: &Cell, _config: &Config) -> squint_eval::Result<Box<dyn Provider>> {
        Ok(Box::new(
            ScriptedProvider::from_path(&root().join("fixtures/scripts/held.json")).unwrap(),
        ))
    }
    fn tools(
        &self,
        selection: &ToolSelection,
        _config: &Config,
    ) -> squint_eval::Result<ToolRegistry> {
        Ok(ToolRegistry::new(vec![], &selection.enabled, &[]).unwrap())
    }
}

fn plan(eval_dir: &Path, cap: f64) -> Plan {
    let matrix_text = std::fs::read_to_string(root().join("fixtures/matrix/fake-a.toml"))
        .unwrap()
        .replace("spend_cap_usd = 5.0", &format!("spend_cap_usd = {cap}"));
    let matrix = Matrix::parse(&matrix_text).unwrap();
    let config_text = std::fs::read_to_string(root().join(&matrix.matrix.config)).unwrap();
    Plan {
        eval_dir: eval_dir.to_path_buf(),
        matrix,
        matrix_text,
        config_text,
        evalsets_root: root().join("fixtures/evalsets"),
        admissions: None,
        harness_version: "0.1.0 (test)".into(),
        allow_fake: true,
    }
}

#[test]
fn each_estimated_envelope_equals_the_ledgers_reservation() {
    let dir = std::env::temp_dir().join(format!("squint-estimate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let p = plan(&dir, 5.0);
    let e = estimate(&p, &Fake).unwrap();
    assert_eq!(e.decisions.len(), 4);
    assert!(!e.over_cap);
    assert!(
        (e.envelope_sum_usd - e.decisions.iter().map(|d| d.envelope_usd).sum::<f64>()).abs()
            < 1e-12
    );
    assert!(e.floor_usd > 0.0 && e.floor_usd < e.envelope_sum_usd);
    run(&p, &Fake, None).unwrap();
    let ledger = std::fs::read_to_string(dir.join("ledger.jsonl")).unwrap();
    let mut reserved = std::collections::BTreeMap::new();
    for line in ledger.lines() {
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        if v["event"] == "reserve" {
            reserved.insert(
                v["id"].as_str().unwrap().to_string(),
                v["usd"].as_f64().unwrap(),
            );
        }
    }
    assert_eq!(reserved.len(), 4);
    for d in &e.decisions {
        let r = reserved[&d.id];
        assert!(
            (r - d.envelope_usd).abs() < 1e-9,
            "{}: estimate {} reserved {r}",
            d.id,
            d.envelope_usd
        );
    }
}

#[test]
fn a_matrix_over_the_cap_warns_but_can_still_complete() {
    let dir = std::env::temp_dir().join(format!("squint-estimate-over-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // Four envelopes of about $0.15 against a $0.25 cap: the sum is over the cap, but the
    // scripted decisions settle to nothing, so every one of them completes.
    let p = plan(&dir, 0.25);
    let e = estimate(&p, &Fake).unwrap();
    assert!(e.over_cap, "{e:?}");
    let s = run(&p, &Fake, None).unwrap();
    assert_eq!(s.completed, 4);
    assert!(s.decisions.iter().all(|d| d.error_class.is_none()), "{s:?}");
}

#[test]
fn a_decision_over_the_per_decision_cap_is_named_and_the_run_refuses_it() {
    let dir = std::env::temp_dir().join(format!("squint-estimate-job-cap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    // The matrix cap is plenty; the config's per-decision cap is below every envelope.
    let mut p = plan(&dir, 50.0);
    p.config_text.push_str("\n[budget]\nspend_cap_usd = 0.01\n");
    let e = estimate(&p, &Fake).unwrap();
    assert!(!e.over_cap);
    assert_eq!(e.over_decision_cap.len(), 4, "{e:?}");
    assert!(e.decisions.iter().all(|d| d.over_decision_cap));
    let s = run(&p, &Fake, None).unwrap();
    assert!(
        s.decisions
            .iter()
            .all(|d| d.error_class.as_deref() == Some("cap")),
        "{s:?}"
    );
    // The same matrix with the cap raised: nothing is flagged.
    let p = plan(&dir.join("ok"), 50.0);
    assert!(estimate(&p, &Fake).unwrap().over_decision_cap.is_empty());
}
