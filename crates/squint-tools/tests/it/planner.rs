//! The envelope planner prices what the model reads of a tool result (its presentation and the
//! returned images), not the stored envelope with its inline pixels: with `view` and `align`
//! enabled the planned decision stays within a few dollars at ordinary prices.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::ledger::Prices;
use squint_core::run;
use squint_core::tool::ToolRegistry;
use squint_tools::builtin;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn config(enabled: &str) -> Config {
    Config::parse(
        &std::fs::read_to_string(root().join("fixtures/config/view.toml"))
            .unwrap()
            .replace("enabled = [\"view\", \"resize\"]", enabled),
        &["budget.spend_cap_usd=100".into()],
        &|_| None,
    )
    .unwrap()
}

#[test]
fn the_planner_prices_the_presentation_the_model_reads_not_the_stored_envelope() {
    let prices = Prices {
        input_per_million: 3.0,
        output_per_million: 15.0,
        per_image: 0.004,
    };
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let none = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let without = run::envelope(&config("enabled = []"), &job, &root(), &none, &prices).unwrap();
    let mut projections = BTreeMap::new();
    projections.insert("align".to_string(), "visual_only".to_string());
    let registry = ToolRegistry::new(builtin::all(), &["view".into(), "align".into()], &[])
        .unwrap()
        .with_projections(&projections)
        .unwrap();
    let config = config("enabled = [\"view\", \"align\"]");
    let with = run::envelope(&config, &job, &root(), &registry, &prices).unwrap();
    assert!(with.usd > without.usd, "{with:?} vs {without:?}");
    // The stored bound of one `align` envelope is six megabytes of pixels, about 1.5 million
    // tokens as text; the planner prices the few kilobytes the model reads instead, so a tool
    // turn adds far less than one such envelope to the history and no request comes near it.
    let turns: Vec<u64> = with
        .requests
        .iter()
        .filter(|r| r.kind == "tool_turn")
        .map(|r| r.shape.input_tokens)
        .collect();
    assert!(turns.len() >= 2, "{with:?}");
    let growth = turns[turns.len() - 1] - turns[turns.len() - 2];
    assert!(growth < 60_000, "{growth} tokens per tool turn: {with:?}");
    let largest = with
        .requests
        .iter()
        .map(|r| r.shape.input_tokens)
        .max()
        .unwrap();
    assert!(
        largest < 250_000,
        "{largest} input tokens planned: {with:?}"
    );
    assert!(
        with.requests.iter().any(|r| r.shape.images > 0),
        "returned images are priced as images: {with:?}"
    );
    assert!(
        with.usd.is_finite() && with.usd < 10.0,
        "a decision with view and align plans at ${:.2}: {with:?}",
        with.usd
    );
}
