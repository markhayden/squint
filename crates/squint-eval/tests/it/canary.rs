//! Task 3.7: the canary against a recorded baseline with the fake provider: `clear`, `suspect`,
//! `recommend_disable`, the reset after requalification, and the refusals.
#![allow(clippy::unwrap_used)]

use super::common;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use common::{root, tempdir};
use squint_core::config::Config;
use squint_core::provider::Provider;
use squint_core::tool::ToolRegistry;
use squint_eval::canary::{self, CanaryState, Judge, Setup, Status};
use squint_eval::sets::Set;
use squint_testkit::ScriptedProvider;

/// Every item is judged by the same script until it is changed.
struct Scripted(Mutex<PathBuf>);

impl Scripted {
    fn new(script: &str) -> Scripted {
        Scripted(Mutex::new(root().join("fixtures/scripts").join(script)))
    }
    fn set(&self, script: &str) {
        *self.0.lock().unwrap() = root().join("fixtures/scripts").join(script);
    }
}

impl Judge for Scripted {
    fn provider(&self, _config: &Config) -> squint_eval::Result<Box<dyn Provider>> {
        Ok(Box::new(
            ScriptedProvider::from_path(&self.0.lock().unwrap()).unwrap(),
        ))
    }
    fn tools(&self, _config: &Config) -> squint_eval::Result<ToolRegistry> {
        Ok(ToolRegistry::new(vec![], &[], &[]).unwrap())
    }
}

fn config(dir: &Path, model: &str) -> Config {
    Config::load_with_overrides(
        &root().join("fixtures/config/minimal.toml"),
        &[
            format!("provider.model={model:?}"),
            format!("cache.dir={:?}", dir.join("cache").display().to_string()),
            "budget.retries=2".into(),
        ],
    )
    .unwrap()
}

fn setup<'a>(
    config: &'a Config,
    set: &'a Set,
    out: &'a Path,
    judge: &'a Scripted,
    cap: f64,
) -> Setup<'a> {
    Setup {
        config,
        set,
        out_dir: out,
        judge,
        harness_version: "test",
        spend_cap_usd: cap,
    }
}

fn set() -> Set {
    Set::load(&root().join("fixtures/evalsets/source-fidelity/pairs-v1")).unwrap()
}

#[test]
fn the_canary_goes_clear_suspect_recommend_disable_and_resets_on_requalification() {
    let dir = tempdir("canary");
    let state = dir.join("canary.toml");
    let out = dir.join("out");
    let cfg = config(&dir, "fake-model");
    let set = set();
    let judge = Scripted::new("held.json");

    // No baseline yet: the canary refuses.
    let e = canary::canary(&state, &setup(&cfg, &set, &out, &judge, 5.0)).unwrap_err();
    assert!(e.to_string().contains("requalify"), "{e}");

    // Requalify without approval writes nothing; with approval records the baseline.
    let proposed = canary::requalify(&state, &setup(&cfg, &set, &out, &judge, 5.0), None).unwrap();
    assert!(!proposed.recorded && !state.exists());
    assert_eq!(proposed.baseline.verdicts["pair-01"], "pass");
    assert_eq!(proposed.baseline.verdicts["pair-02"], "pass");
    let q = canary::requalify(
        &state,
        &setup(&cfg, &set, &out, &judge, 5.0),
        Some("owner: qualified"),
    )
    .unwrap();
    assert!(q.recorded && q.changed.is_empty());
    let s = CanaryState::load(&state).unwrap();
    assert_eq!(s.baseline.as_ref().unwrap().id, q.baseline.id);
    assert_eq!(s.baseline.as_ref().unwrap().approval, "owner: qualified");
    assert_eq!(
        s.baseline.as_ref().unwrap().configuration,
        canary::configuration_identity(&cfg, &judge, "test").unwrap()
    );
    assert!(s.runs.is_empty());

    // Same verdicts: clear, and never from the cache.
    let r = canary::canary(&state, &setup(&cfg, &set, &out, &judge, 5.0)).unwrap();
    assert_eq!(r.status, Status::Clear);
    assert!(r.deviations.is_empty());
    let mut reports = 0;
    for entry in std::fs::read_dir(&out).unwrap() {
        let p = entry.unwrap().path().join("report.json");
        if let Ok(text) = std::fs::read_to_string(&p) {
            let v: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(v["cache"]["status"], "bypass", "{}", p.display());
            reports += 1;
        }
    }
    assert!(
        reports >= 4,
        "two requalifications and a canary over two items"
    );
    // An operational error is never a verdict: nothing deviates, the run is inconclusive, and a
    // requalification refuses to record it.
    judge.set("retries-exhausted.json");
    let r = canary::canary(&state, &setup(&cfg, &set, &out, &judge, 5.0)).unwrap();
    assert_eq!(r.status, Status::Inconclusive);
    assert!(
        r.deviations.is_empty() && r.errors == ["pair-01", "pair-02"],
        "{r:?}"
    );
    let e =
        canary::requalify(&state, &setup(&cfg, &set, &out, &judge, 5.0), Some("x")).unwrap_err();
    assert!(e.to_string().contains("operational error"), "{e}");
    judge.set("held.json");
    assert_eq!(
        canary::canary(&state, &setup(&cfg, &set, &out, &judge, 5.0))
            .unwrap()
            .status,
        Status::Clear
    );
    // A deviation: suspect. Another against the same baseline: recommend_disable.
    judge.set("violation.json");
    let r = canary::canary(&state, &setup(&cfg, &set, &out, &judge, 5.0)).unwrap();
    assert_eq!(r.status, Status::Suspect);
    assert_eq!(r.deviations.len(), 2);
    assert_eq!(
        (
            r.deviations[0].baseline.as_str(),
            r.deviations[0].observed.as_str()
        ),
        ("pass", "fail")
    );
    let r = canary::canary(&state, &setup(&cfg, &set, &out, &judge, 5.0)).unwrap();
    assert_eq!(r.status, Status::RecommendDisable);
    // Nothing was disabled: the state only records.
    let s = CanaryState::load(&state).unwrap();
    assert_eq!(s.runs.len(), 5);
    assert_eq!(
        s.runs.iter().map(|r| r.status).collect::<Vec<_>>(),
        [
            Status::Clear,
            Status::Inconclusive,
            Status::Clear,
            Status::Suspect,
            Status::RecommendDisable
        ]
    );
    // Back in line: clear again, and the next deviation is suspect, not recommend_disable.
    judge.set("held.json");
    assert_eq!(
        canary::canary(&state, &setup(&cfg, &set, &out, &judge, 5.0))
            .unwrap()
            .status,
        Status::Clear
    );
    judge.set("violation.json");
    assert_eq!(
        canary::canary(&state, &setup(&cfg, &set, &out, &judge, 5.0))
            .unwrap()
            .status,
        Status::Suspect
    );

    // Requalification on the new behaviour: the baseline changes, the history is cleared, and
    // the canary is clear against it.
    let q = canary::requalify(
        &state,
        &setup(&cfg, &set, &out, &judge, 5.0),
        Some("owner: new baseline"),
    )
    .unwrap();
    assert!(q.recorded);
    assert_eq!(
        q.changed.len(),
        2,
        "both verdicts changed from the old baseline"
    );
    let s = CanaryState::load(&state).unwrap();
    assert!(s.runs.is_empty());
    assert_eq!(s.baseline.as_ref().unwrap().verdicts["pair-01"], "fail");
    assert_eq!(
        canary::canary(&state, &setup(&cfg, &set, &out, &judge, 5.0))
            .unwrap()
            .status,
        Status::Clear
    );

    // A changed configuration is not what the baseline covers.
    let other = config(&dir, "fake-model-2");
    assert_ne!(
        canary::configuration_identity(&other, &judge, "test").unwrap(),
        canary::configuration_identity(&cfg, &judge, "test").unwrap()
    );
    let e = canary::canary(&state, &setup(&other, &set, &out, &judge, 5.0)).unwrap_err();
    assert!(e.to_string().contains("configuration differs"), "{e}");
    // The configuration identity ignores what does not decide an answer.
    let cheaper = Config::load_with_overrides(
        &root().join("fixtures/config/minimal.toml"),
        &[
            "budget.spend_cap_usd=1.0".into(),
            "budget.retries=2".into(),
            format!("cache.dir={:?}", dir.join("cache").display().to_string()),
        ],
    )
    .unwrap();
    assert_eq!(
        canary::configuration_identity(&cheaper, &judge, "test").unwrap(),
        canary::configuration_identity(&cfg, &judge, "test").unwrap()
    );
    // The harness version is part of it; and a spend cap that cannot cover one item refuses
    // before anything is sent.
    assert_ne!(
        canary::configuration_identity(&cfg, &judge, "other").unwrap(),
        canary::configuration_identity(&cfg, &judge, "test").unwrap()
    );
    let e = canary::canary(&state, &setup(&cfg, &set, &out, &judge, 0.001)).unwrap_err();
    assert!(
        e.to_string().contains("spend cap") && e.to_string().contains("nothing more was sent"),
        "{e}"
    );
}
