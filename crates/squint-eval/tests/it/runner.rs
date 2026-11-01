//! Task 3.4b: the matrix runner with the fake provider: every cell runs, the cache is never
//! read, the cap stops it with `error: budget` per unstarted decision, a cancelled run resumes
//! without redoing complete decisions and marks partial ones, repetition 1 is the decision
//! repetition, and prompts across arms differ only in the Tools section.
#![allow(clippy::unwrap_used)]

use super::common;

use std::path::Path;

use common::{Fake, plan, root, tempdir};
use squint_core::run::Cancel;
use squint_core::tool::{Tool, ToolRegistry};
use squint_eval::runner::{Summary, run};
use squint_testkit::ScriptedTool;

fn fake(script: &str) -> Fake {
    Fake::new(script)
}

fn summary(dir: &Path) -> Summary {
    serde_json::from_str(&std::fs::read_to_string(dir.join("summary.json")).unwrap()).unwrap()
}

#[test]
fn every_cell_runs_with_the_cache_bypassed_and_repetition_one_flagged() {
    let dir = tempdir("cells");
    let s = run(&plan(&dir, "fake-a.toml"), &fake("held.json"), None).unwrap();
    assert_eq!(s.decisions.len(), 4, "2 items × 1 cell × 2 repetitions");
    assert_eq!(s.completed, 4);
    assert_eq!(s.skipped, 0);
    assert!(s.partial.is_empty());
    for d in &s.decisions {
        assert_eq!(d.verdict.as_deref(), Some("pass"), "{d:?}");
        assert_eq!(d.exit_code, 0);
        assert_eq!(d.cell, "fake:fake-model/A");
        assert_eq!(d.decision_repetition, d.repetition == 1);
        let report: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join(&d.dir).join("report.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            report["cache"]["status"], "bypass",
            "an evaluation never reads the cache"
        );
        assert!(d.wall_ms > 0 || d.cost_usd >= 0.0);
    }
    assert_eq!(summary(&dir), s);
    assert!(dir.join("descriptor.json").exists() && dir.join("ledger.jsonl").exists());
    // The lock file stays (the OS lock on it is what is held) and is free again.
    drop(squint_eval::descriptor::WriterLock::acquire(&dir).unwrap());
    // The run ends with the metrics and the Markdown report, which `eval report` reproduces
    // byte for byte from the saved results and the labels.
    let report_md = std::fs::read_to_string(dir.join("report.md")).unwrap();
    let metrics_json = std::fs::read_to_string(dir.join("metrics.json")).unwrap();
    let m: squint_eval::metrics::MatrixMetrics = serde_json::from_str(&metrics_json).unwrap();
    assert_eq!(m.cells.len(), 1);
    let c = &m.cells[0];
    assert_eq!(
        (
            c.counts.items,
            c.counts.expected_fail,
            c.counts.expected_pass
        ),
        (2, 1, 1)
    );
    assert_eq!(
        c.counts.missed, 1,
        "pair-02 is a rotation the scripted pass misses"
    );
    assert_eq!(c.repetitions.count, 2);
    assert_eq!(c.repetitions.agreement, Some(1.0));
    assert!(
        report_md.starts_with("# Evaluation report: fake-a\n"),
        "{report_md}"
    );
    std::fs::remove_file(dir.join("report.md")).unwrap();
    squint_eval::summary::regenerate(&dir, &root().join("fixtures/evalsets")).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("report.md")).unwrap(),
        report_md
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("metrics.json")).unwrap(),
        metrics_json
    );
    // The same matrix again: everything is complete, nothing is redone.
    let again = run(&plan(&dir, "fake-a.toml"), &fake("held.json"), None).unwrap();
    assert_eq!((again.completed, again.skipped), (0, 4));
}

#[test]
fn the_cap_stops_the_matrix_before_a_decision_it_cannot_cover_and_records_nothing_for_it() {
    let dir = tempdir("cap");
    let mut p = plan(&dir, "fake-a.toml");
    p.matrix.matrix.spend_cap_usd = 0.2;
    p.matrix_text = p
        .matrix_text
        .replace("spend_cap_usd = 5.0", "spend_cap_usd = 0.2");
    let s = run(&p, &fake("held-costly.json"), None).unwrap();
    // The first decision fits and settles to about $0.09; the second's envelope no longer does.
    // The run stops there: no row, no directory, nothing sent, and the summary says why.
    assert!(s.budget_exhausted, "{s:?}");
    assert_eq!(
        s.decisions
            .iter()
            .map(|d| d.id.as_str())
            .collect::<Vec<_>>(),
        ["source-fidelity/pairs-v1/pair-01/fake:fake-model/A/rep-1"]
    );
    assert_eq!(s.decisions[0].verdict.as_deref(), Some("pass"));
    assert!(s.decisions.iter().all(|d| d.error_class.is_none()));
    assert!(
        !dir.join("results/source-fidelity/pairs-v1/pair-01/fake_fake-model/A/rep-2")
            .exists()
    );
    assert!(s.spent_usd <= 0.2 + 1e-9);
    assert!(!dir.join("LOCK").exists() || std::fs::read_to_string(dir.join("LOCK")).is_ok());
    // Resuming under the same cap stops at the same place; the complete decision is kept.
    let again = run(&p, &fake("held-costly.json"), None).unwrap();
    assert!(again.budget_exhausted);
    assert_eq!((again.skipped, again.completed), (1, 0));
}

#[test]
fn a_changed_admission_record_changes_arm_d_and_refuses_resume() {
    use squint_eval::admissions::Admissions;
    let dir = tempdir("arm-d");
    let mut p = plan(&dir, "fake-a.toml");
    p.matrix_text = p.matrix_text.replace("arms = [\"A\"]", "arms = [\"D\"]");
    p.matrix = squint_eval::matrix::Matrix::parse(&p.matrix_text).unwrap();
    p.admissions =
        Some(Admissions::load(&root().join("fixtures/evalsets/admissions.toml")).unwrap());
    let fake_d = || {
        let f = Fake::new("held.json");
        *f.tool_envelopes.lock().unwrap() =
            Some(root().join("fixtures/tool-scripts/regions-25.json"));
        f
    };
    let s = run(&p, &fake_d(), None).unwrap();
    assert_eq!(s.completed, 4);
    let d: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("descriptor.json")).unwrap())
            .unwrap();
    assert_eq!(
        d["selections"]["D@source-fidelity"],
        serde_json::json!(["view", "align:visual_only", "classes"])
    );
    // The confirmed tool loses its admission: arm D is another arm now, and the directory refuses.
    let mut p2 = plan(&dir, "fake-a.toml");
    p2.matrix_text = p.matrix_text.clone();
    p2.matrix = p.matrix.clone();
    p2.admissions = Some(
        Admissions::parse(
            &std::fs::read_to_string(root().join("fixtures/evalsets/admissions.toml"))
                .unwrap()
                .replace("stage = \"confirmed\"", "stage = \"provisional\""),
        )
        .unwrap(),
    );
    let err = run(&p2, &fake_d(), None).unwrap_err().to_string();
    assert!(
        err.contains("different inputs") && err.contains("arm selections"),
        "{err}"
    );
}

#[test]
fn a_cancelled_run_resumes_without_redoing_complete_decisions_and_marks_partial_ones() {
    let dir = tempdir("resume");
    let cancel = Cancel::new();
    let mut f = fake("held.json");
    f.cancel_after = Some((2, cancel.clone()));
    let s = run(&plan(&dir, "fake-a.toml"), &f, Some(&cancel)).unwrap();
    assert!(s.cancelled, "{s:?}");
    assert_eq!(s.completed, 2);
    // Decisions run item by item, then cell, then repetition: the third is pair-02's first.
    // Fake a crash mid-decision: its directory holds a started attempt and no result.
    let third = dir.join("results/source-fidelity/pairs-v1/pair-02/fake_fake-model/A/rep-1");
    assert!(!third.join("result.json").exists());
    std::fs::create_dir_all(third.join("decision-000-crashed/run-1")).unwrap();
    let s2 = run(&plan(&dir, "fake-a.toml"), &fake("held.json"), None).unwrap();
    assert!(!s2.cancelled);
    assert_eq!(s2.skipped, 2, "the two complete decisions are not redone");
    assert_eq!(s2.completed, 2);
    assert_eq!(
        s2.partial,
        ["source-fidelity/pairs-v1/pair-02/fake:fake-model/A/rep-1"]
    );
    assert_eq!(
        s2.decisions.len(),
        4,
        "the summary covers every decision, resumed ones included"
    );
}

#[test]
fn a_changed_input_refuses_resume() {
    let dir = tempdir("changed");
    run(&plan(&dir, "fake-a.toml"), &fake("held.json"), None).unwrap();
    let mut p = plan(&dir, "fake-a.toml");
    p.harness_version = "0.9.9".into();
    let err = run(&p, &fake("held.json"), None).unwrap_err();
    assert!(err.to_string().contains("different inputs"), "{err}");
}

#[test]
fn prompts_across_arms_differ_only_in_the_tools_section() {
    use squint_core::job::Job;
    use squint_core::obligations::{MergeInput, merge, resolve};
    use squint_core::render::{system_prompt, tools_section};
    let job = Job::load(
        &root().join("fixtures/evalsets/source-fidelity/pairs-v1/items/pair-01/job.toml"),
    )
    .unwrap();
    let merged = merge(&MergeInput {
        base: "source-fidelity",
        profile: None,
        extra: &[],
        extra_names: &[],
        job: Some(&job),
        enabled_tools: &[],
    })
    .unwrap();
    let resolved = resolve(&merged, &job).unwrap();
    let none = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let text = std::fs::read_to_string(root().join("fixtures/tool-scripts/regions-3-then-1.json"))
        .unwrap();
    let with: Vec<Box<dyn Tool>> = vec![Box::new(ScriptedTool::fake_regions(
        serde_json::from_str(&text).unwrap(),
    ))];
    let with = ToolRegistry::new(with, &["fake_regions".into()], &[]).unwrap();
    let a = system_prompt(&resolved, &job, none.manifests());
    let d = system_prompt(&resolved, &job, with.manifests());
    assert_ne!(a, d);
    let a_stripped = a.replace(&tools_section(none.manifests()), "");
    let d_stripped = d.replace(&tools_section(with.manifests()), "");
    assert_eq!(
        a_stripped, d_stripped,
        "everything but the Tools section is identical"
    );
}

#[test]
fn a_model_account_in_the_matrix_binds_its_cells() {
    let dir = tempdir("account");
    let mut p = plan(&dir, "fake-a.toml");
    p.matrix_text = p.matrix_text.replace(
        "model = \"fake-model\"",
        "model = \"fake-model\"\naccount = \"fake:from-the-matrix\"",
    );
    p.matrix = squint_eval::matrix::Matrix::parse(&p.matrix_text).unwrap();
    let s = run(&p, &fake("held.json"), None).unwrap();
    let report: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join(&s.decisions[0].dir).join("report.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        report["config"]["privacy"]["account"],
        "fake:from-the-matrix"
    );
}

#[test]
fn a_completed_error_is_kept_on_resume() {
    let dir = tempdir("errors-kept");
    let errors = || {
        let mut p = plan(&dir, "fake-a.toml");
        p.config_text.push_str("\n[budget]\nretries = 2\n");
        p
    };
    let s = run(&errors(), &fake("retries-exhausted.json"), None).unwrap();
    assert_eq!(s.completed, 4);
    assert!(s.decisions.iter().all(|d| d.error_class.is_some()), "{s:?}");
    let class = s.decisions[0].error_class.clone();
    // Resume with a script that would pass: nothing is redone and the errors stay, with their class.
    let again = run(&errors(), &fake("held.json"), None).unwrap();
    assert_eq!((again.completed, again.skipped), (0, 4));
    assert!(
        again
            .decisions
            .iter()
            .all(|d| d.error_class == class && d.verdict.is_none())
    );
}

// ---- added at the M3 baseline: the first live run scored no localisation ---------------------

/// The prompt asks for a location on every checklist item marked `violation`, so those items are
/// the localisation predictions alongside violation findings. `pair-02` is labelled a whole-image
/// rotation under SF2 and the scripted submission marks `chk:ob:SF2@candidate` a whole-image
/// violation: one labelled region, one prediction, one hit. The scripted `pair-01` violation is
/// a false reject and never a localisation prediction.
#[test]
fn checklist_item_violations_are_localisation_predictions() {
    let dir = tempdir("item-localisation");
    run(&plan(&dir, "fake-a.toml"), &fake("violation.json"), None).unwrap();
    let localisation = |dir: &Path| {
        let m: squint_eval::metrics::MatrixMetrics =
            serde_json::from_str(&std::fs::read_to_string(dir.join("metrics.json")).unwrap())
                .unwrap();
        m.cells[0].localisation.clone()
    };
    let expected = squint_eval::metrics::Localisation {
        labelled_regions: 1,
        hits: 1,
        predictions: 1,
        spurious: 0,
    };
    assert_eq!(localisation(&dir), expected);
    // A location on a raw frame is already in root coordinates: the frames table names the raw
    // frame's image, so a report without `root` on its items scores the same.
    for d in &summary(&dir).decisions {
        let path = dir.join(&d.dir).join("report.json");
        let mut report: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        for item in report["runs"][0]["items"].as_array_mut().unwrap() {
            assert!(
                item.as_object_mut().unwrap().remove("root").is_some()
                    || item["location"].is_null()
            );
        }
        std::fs::write(&path, serde_json::to_string(&report).unwrap()).unwrap();
    }
    squint_eval::summary::regenerate(&dir, &root().join("fixtures/evalsets")).unwrap();
    assert_eq!(localisation(&dir), expected);
}

#[test]
fn the_summary_records_decision_directories_relative_to_the_evaluation_directory() {
    // The run makes a decision directory absolute (a pack is restarted in it); the evaluation
    // directory is whatever the command line said. Either way the summary carries no local path.
    let relative = Path::new("eval-out/some-run");
    let absolute = std::path::absolute(relative).unwrap();
    let decision = absolute.join("results/set/item/cell/A/rep-1/decision-1-2");
    assert_eq!(
        squint_eval::runner::relative_dir(&decision, relative),
        "results/set/item/cell/A/rep-1/decision-1-2"
    );
    assert_eq!(
        squint_eval::runner::relative_dir(&decision, &absolute),
        "results/set/item/cell/A/rep-1/decision-1-2"
    );
    assert_eq!(
        squint_eval::runner::relative_dir(Path::new("results/x"), relative),
        "results/x"
    );
}
