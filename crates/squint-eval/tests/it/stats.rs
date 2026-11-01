//! Task 1.15: the stored resample indices and interval are reproduced exactly from the stored
//! seed; the hand-checkable facts of the fixture hold; four differing families and a thin tail
//! each yield `inconclusive`.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_eval::fixture::{Golden, StatsFixture};
use squint_eval::gate_rules::Outcome;

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/stats/case-1.toml")
}

#[test]
fn stats_fixture_hand_checkable_facts_hold() {
    let f = StatsFixture::load(&fixture_path()).unwrap();
    let e = f.evaluate().unwrap();
    assert_eq!(
        (e.c.expected_fail, e.c.detected, e.c.missed, e.c.unsure),
        (8, 1, 6, 1)
    );
    assert_eq!(
        (
            e.x.expected_fail,
            e.x.detected,
            e.x.missed,
            e.x.false_rejects,
            e.x.errors
        ),
        (8, 7, 1, 1, 0)
    );
    assert!((e.bootstrap.point - 0.75).abs() < 1e-12);
    assert_eq!(e.bootstrap.families, 6, "f7 has no expected fail");
    assert_eq!(e.bootstrap.differing_families, 5);
    assert!(!e.bootstrap.insufficient_tail);
    assert!(e.bootstrap.lower > 0.0 && e.bootstrap.lower <= e.bootstrap.point);
    assert!(e.bootstrap.upper >= e.bootstrap.point && e.bootstrap.upper <= 1.0);
    assert_eq!(
        e.verdict.outcome,
        Outcome::Admitted,
        "{:?}",
        e.verdict.failed
    );
}

#[test]
fn stats_golden_resample_indices_and_interval_are_reproduced_from_the_seed() {
    let path = fixture_path();
    let f = StatsFixture::load(&path).unwrap();
    let e = f.evaluate().unwrap();
    let golden = StatsFixture::golden_of(&e);
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        let mut text = std::fs::read_to_string(&path).unwrap();
        if let Some(i) = text.find("\n[golden]") {
            text.truncate(i + 1);
        }
        text.push_str(&format!(
            "\n[golden]\n{}",
            toml::to_string(&golden).unwrap()
        ));
        std::fs::write(&path, text).unwrap();
    }
    let stored: Golden = StatsFixture::load(&path)
        .unwrap()
        .golden
        .expect("run with UPDATE_FIXTURES=1 once");
    assert_eq!(
        stored.first_resamples, golden.first_resamples,
        "the seed reproduces the indices exactly"
    );
    assert_eq!(stored.point.to_bits(), golden.point.to_bits());
    assert_eq!(stored.lower.to_bits(), golden.lower.to_bits());
    assert_eq!(stored.upper.to_bits(), golden.upper.to_bits());
    assert_eq!(stored.differing_families, golden.differing_families);
    assert_eq!(stored.outcome, golden.outcome);
    assert_eq!(golden.first_resamples.len(), 3);
    assert!(
        golden
            .first_resamples
            .iter()
            .all(|r| r.len() == 6 && r.iter().all(|&i| i < 6))
    );
}

#[test]
fn stats_four_differing_families_are_inconclusive() {
    let mut f = StatsFixture::load(&fixture_path()).unwrap();
    f.items.retain(|i| i.family != "f6");
    let e = f.evaluate().unwrap();
    assert_eq!(e.bootstrap.differing_families, 4);
    assert_eq!(e.verdict.outcome, Outcome::Inconclusive);
    assert!(
        e.verdict
            .failed
            .iter()
            .any(|r| r.contains("independent families")),
        "{:?}",
        e.verdict.failed
    );
}

#[test]
fn stats_a_tail_with_fewer_than_twenty_resamples_is_inconclusive() {
    let mut f = StatsFixture::load(&fixture_path()).unwrap();
    f.resamples = 400;
    let e = f.evaluate().unwrap();
    assert!(
        e.bootstrap.insufficient_tail,
        "10 resamples below the 2.5% percentile"
    );
    assert_eq!(e.verdict.outcome, Outcome::Inconclusive);
    f.resamples = 1000;
    f.hypotheses = 4;
    let e = f.evaluate().unwrap();
    assert!(e.bootstrap.insufficient_tail, "6.25 resamples below 0.625%");
    assert_eq!(e.verdict.outcome, Outcome::Inconclusive);
}
