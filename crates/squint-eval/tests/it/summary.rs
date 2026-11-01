//! Task 3.5a: the Markdown report is a pure function of the summary and the metrics, checked
//! against a stored rendering of the hand-computed fixture (`UPDATE_GOLDEN=1` rewrites it).
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_eval::gate_rules::{Computed, Policy};
use squint_eval::metrics::{Expected, ItemRow, Region, compute};
use squint_eval::runner::Summary;
use squint_eval::summary::render;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[derive(serde::Deserialize)]
struct Fixture {
    cell: String,
    policy: Policy,
    rows: Vec<FixtureRow>,
}

#[derive(serde::Deserialize)]
struct FixtureRow {
    item: String,
    repetition: u32,
    expected: Expected,
    computed: Computed,
    cost_usd: f64,
    wall_ms: u64,
    #[serde(default)]
    labels: Vec<Region>,
    #[serde(default)]
    predictions: Vec<Region>,
}

#[test]
fn the_report_renders_the_fixture_exactly_as_stored() {
    let text = std::fs::read_to_string(root().join("fixtures/stats/metrics-1.toml")).unwrap();
    let f: Fixture = toml::from_str(&text).unwrap();
    let rows: Vec<ItemRow> = f
        .rows
        .iter()
        .map(|r| ItemRow {
            set: "fixture".into(),
            item: r.item.clone(),
            family: String::new(),
            cell: f.cell.clone(),
            repetition: r.repetition,
            expected: r.expected,
            computed: r.computed,
            cost_usd: r.cost_usd,
            wall_ms: r.wall_ms,
            labels: r.labels.clone(),
            predictions: r.predictions.clone(),
        })
        .collect();
    let m = compute(&rows, &f.policy);
    let summary = Summary {
        summary_version: 1,
        matrix: "metrics-1".into(),
        matrix_identity: "0".repeat(64),
        descriptor_sha256: "1".repeat(64),
        decisions: Vec::new(),
        completed: 10,
        skipped: 0,
        partial: Vec::new(),
        cancelled: false,
        budget_exhausted: false,
        spent_usd: 1.5,
        cap_usd: 5.0,
        bootstrap: None,
    };
    let rendered = render(&summary, &m);
    let golden = root().join("fixtures/stats/metrics-1.md");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden, &rendered).unwrap();
    }
    let stored = std::fs::read_to_string(&golden).unwrap();
    assert_eq!(
        rendered, stored,
        "render differs from fixtures/stats/metrics-1.md"
    );
    assert!(rendered.contains("| Detected failures | 1 / 3 | 0.333 |"));
    assert!(rendered.contains("2 hits of 2 labelled regions, 1 spurious of 3 predictions"));
    assert!(rendered.contains("agree with repetition 1 on 0.500 of items"));
    // The same inputs render the same bytes.
    assert_eq!(render(&summary, &m), rendered);
}

// ---- task 4.5: the paired section of the report ------------------------------------------------

#[test]
fn the_report_lists_paired_flips_and_the_interval_when_two_arms_are_present() {
    let row = |cell: &str, item: &str, expected: Expected, computed: Computed| ItemRow {
        set: "s".into(),
        item: item.into(),
        family: "f".into(),
        cell: cell.into(),
        repetition: 1,
        expected,
        computed,
        cost_usd: 0.0,
        wall_ms: 1,
        labels: Vec::new(),
        predictions: Vec::new(),
    };
    let rows = vec![
        row("fake:m/A", "i1", Expected::Fail, Computed::Pass),
        row("fake:m/C", "i1", Expected::Fail, Computed::Fail),
        row("fake:m/A", "i2", Expected::Pass, Computed::Pass),
        row("fake:m/C", "i2", Expected::Pass, Computed::Pass),
    ];
    let m = squint_eval::metrics::compute_with(
        &rows,
        &squint_eval::summary::policy_of(&squint_core::config::PolicyConfig::default()),
        Some(squint_eval::metrics::BootstrapSpec {
            seed: 3,
            resamples: 500,
            hypotheses: 1,
        }),
    );
    let summary = Summary {
        summary_version: 1,
        matrix: "paired".into(),
        matrix_identity: "0".repeat(64),
        descriptor_sha256: "1".repeat(64),
        decisions: Vec::new(),
        completed: 4,
        skipped: 0,
        partial: Vec::new(),
        cancelled: false,
        budget_exhausted: false,
        spent_usd: 0.1,
        cap_usd: 1.0,
        bootstrap: Some(squint_eval::metrics::BootstrapSpec {
            seed: 3,
            resamples: 500,
            hypotheses: 1,
        }),
    };
    let text = render(&summary, &m);
    assert!(
        text.contains("## Paired: fake:m under C against A"),
        "{text}"
    );
    assert!(text.contains("2 items scored under both arms (repetition 1); 1 flipped. Detected failures: 1 gained, 0 lost."), "{text}");
    assert!(
        text.contains("seed 3, 500 resamples over 1 families, 1 hypothesis: point 1.0000"),
        "{text}"
    );
    assert!(text.contains("| s/i1 | Fail | Pass | Fail |"), "{text}");
    assert_eq!(text, render(&summary, &m), "byte-stable");
}
