//! Task 3.5b: the gate lifecycle over synthetic sealed sets run with the fake provider: the
//! initial stage on a blind set (receipt, publication, provisional admission, resume), every
//! refusal, the confirmation stage, repeat scoring under an ADR reference, and the threshold
//! boundaries of the gate table.
#![allow(clippy::unwrap_used)]

use super::common;

use std::path::{Path, PathBuf};

use common::{Chooser, Fake, build_set, plan_from_text, root, tempdir};
use squint_core::config::PolicyOutcome;
use squint_eval::admissions::{Admissions, Stage};
use squint_eval::bootstrap::Bootstrap;
use squint_eval::gate::{self, GateReport, GateStage, Receipt, Request};
use squint_eval::gate_rules::{ArmMetrics, Outcome, Policy, Thresholds, decide};
use squint_eval::matrix::{Arm, Cell, Matrix};
use squint_eval::runner::run;
use squint_eval::sets::{self, Set};

fn matrix_text(set: &str, cap: f64) -> String {
    format!(
        "[matrix]\nname = \"gate-fake\"\nconfig = \"fixtures/config/minimal.toml\"\nsets = [{set:?}]\narms = [\"C\", \"D_fake_regions\"]\ndecision_runs = 1\nrepetitions = 1\nspend_cap_usd = {cap}\n\n[matrix.gate]\nseed = 7\nresamples = 1000\n\n[[matrix.models]]\nprofile = \"fake\"\nmodel = \"fake-model\"\n"
    )
}

/// Arm C passes everything; the tool arm fails the two expected-fail items of every family and
/// passes the third. Decisions run item by item, C then D_fake_regions.
fn chooser() -> Chooser {
    Box::new(|n, cell: &Cell| {
        let item = (n - 1) / 2;
        let detect = matches!(cell.arm, Arm::DX(_)) && item % 3 != 2;
        root().join("fixtures/scripts").join(if detect {
            "violation.json"
        } else {
            "held.json"
        })
    })
}

struct World {
    dir: PathBuf,
    evalsets: PathBuf,
    admissions: PathBuf,
    runs: usize,
}

impl World {
    fn new(tag: &str) -> World {
        let dir = tempdir(&format!("gate-{tag}"));
        let evalsets = dir.join("evalsets");
        std::fs::create_dir_all(&evalsets).unwrap();
        World {
            admissions: evalsets.join("admissions.toml"),
            dir,
            evalsets,
            runs: 0,
        }
    }

    /// A sealed blind set; returns the owner-held label file.
    fn blind(&self, id: &str, prefix: &str, families: usize) -> PathBuf {
        let set_dir = build_set(&self.evalsets, id, "blind", prefix, families);
        let owner = self.dir.join("owner").join(id.replace('/', "_"));
        sets::seal(&set_dir, "no judge or tool has seen these", Some(&owner)).unwrap();
        owner.join("labels.toml")
    }

    fn sealed(&self, id: &str, split: &str, prefix: &str, families: usize) {
        let set_dir = build_set(&self.evalsets, id, split, prefix, families);
        sets::seal(&set_dir, "built for the tests", None).unwrap();
    }

    /// Runs the matrix text into a fresh evaluation directory and returns it.
    fn run(&mut self, text: &str, config_extra: &str) -> PathBuf {
        self.runs += 1;
        let eval_dir = self.dir.join(format!("eval-{}", self.runs));
        // One cache directory for the world: the config text, and so the judging digest, is the
        // same across its runs, as it would be for one matrix file.
        let mut p = plan_from_text(&eval_dir, &self.evalsets, text, &self.dir.join("cache"));
        p.config_text.push_str(config_extra);
        let s = run(&p, &Fake::choosing(chooser()), None).unwrap();
        assert!(
            !s.cancelled && s.decisions.iter().all(|d| d.error_class.is_none()),
            "{s:?}"
        );
        eval_dir
    }

    #[allow(clippy::too_many_arguments)]
    fn gate(
        &self,
        eval_dir: &Path,
        text: &str,
        set: &str,
        stage: GateStage,
        labels: Option<&Path>,
        initial: Option<&str>,
        adr: Option<&str>,
    ) -> squint_eval::Result<GateReport> {
        let matrix = Matrix::parse(text).unwrap();
        gate::gate(&Request {
            eval_dir,
            matrix: &matrix,
            evalsets_root: &self.evalsets,
            set,
            tool: "fake_regions",
            model: None,
            stage,
            labels,
            initial_receipt: initial,
            adr,
            admissions: &self.admissions,
        })
    }
}

fn err(r: squint_eval::Result<GateReport>) -> String {
    r.err()
        .map(|e| e.to_string())
        .unwrap_or_else(|| "no error".into())
}

#[test]
fn the_initial_stage_scores_a_blind_set_publishes_its_labels_and_resumes_from_the_receipt() {
    let mut w = World::new("initial");
    let set = "source-fidelity/blind-v1";
    let labels = w.blind(set, "fam", 8);
    let text = matrix_text(set, 50.0);
    let eval_dir = w.run(&text, "");

    // Refusals before any scoring: a label file that is not the sealed one, a matrix whose
    // identity is not the one the results were produced under.
    let tampered = w.dir.join("tampered-labels.toml");
    std::fs::write(
        &tampered,
        std::fs::read_to_string(&labels).unwrap() + "# edited\n",
    )
    .unwrap();
    let e = err(w.gate(
        &eval_dir,
        &text,
        set,
        GateStage::Initial,
        Some(&tampered),
        None,
        None,
    ));
    assert!(e.contains("label-hash mismatch"), "{e}");
    let other = matrix_text(set, 60.0);
    let e = err(w.gate(
        &eval_dir,
        &other,
        set,
        GateStage::Initial,
        Some(&labels),
        None,
        None,
    ));
    assert!(e.contains("descriptor mismatch"), "{e}");
    let e = err(w.gate(&eval_dir, &text, set, GateStage::Initial, None, None, None));
    assert!(e.contains("--labels"), "{e}");

    let r = w
        .gate(
            &eval_dir,
            &text,
            set,
            GateStage::Initial,
            Some(&labels),
            None,
            None,
        )
        .unwrap();
    assert!(!r.resumed);
    let b = &r.receipt.receipt;
    assert_eq!(b.outcome, Outcome::Admitted, "{:?}", b.failed);
    assert_eq!(
        (b.items, b.c.detected, b.x.detected, b.x.false_rejects),
        (24, 0, 16, 0)
    );
    assert_eq!(b.bootstrap.differing_families, 8);
    assert!(b.bootstrap.lower > 0.0);
    assert_eq!(b.stage, GateStage::Initial);
    assert_eq!(b.model, "fake:fake-model");
    let receipt_path = gate::receipt_path(&w.evalsets, &b.id);
    assert!(receipt_path.exists());
    assert_eq!(Receipt::load(&receipt_path).unwrap(), r.receipt);
    // Publication: the labels sit beside the manifest, the split is heldout, the seal holds.
    let set_dir = w.evalsets.join(set);
    assert_eq!(
        std::fs::read(set_dir.join("labels.toml")).unwrap(),
        std::fs::read(&labels).unwrap()
    );
    let manifest = std::fs::read_to_string(set_dir.join("set.toml")).unwrap();
    assert!(
        manifest.contains("split = \"heldout\"") && manifest.contains("labels = \"labels.toml\"")
    );
    assert!(!manifest.contains("labels_sha256"));
    sets::verify_seal(&set_dir).unwrap();
    Set::load(&set_dir).unwrap().check().unwrap();
    assert!(r.written.iter().any(|p| p.ends_with("set.toml")));
    // Admission, read back by the 3.4a reader.
    let a = Admissions::load(&w.admissions).unwrap();
    assert_eq!(
        a.stage("fake_regions", "source-fidelity"),
        Some(Stage::Provisional)
    );
    assert_eq!(a.admission[0].receipt, b.id);
    assert_eq!(r.admission, Some(Stage::Provisional));
    assert!(
        a.admitted("source-fidelity").is_empty(),
        "provisional is not confirmed"
    );

    // The same gate again resumes from the receipt: nothing rescored, nothing changed.
    let again = w
        .gate(
            &eval_dir,
            &text,
            set,
            GateStage::Initial,
            Some(&labels),
            None,
            None,
        )
        .unwrap();
    assert!(again.resumed);
    assert_eq!(again.receipt, r.receipt);
    assert_eq!(Admissions::load(&w.admissions).unwrap(), a);

    // A new run of the same set is a repeat scoring: refused without an ADR reference, and a
    // published set is no longer blind.
    let text2 = matrix_text(set, 60.0);
    let eval2 = w.run(&text2, "");
    let e = err(w.gate(
        &eval2,
        &text2,
        set,
        GateStage::Initial,
        Some(&labels),
        None,
        None,
    ));
    assert!(e.contains("already scored") && e.contains("--adr"), "{e}");
    let e = err(w.gate(
        &eval2,
        &text2,
        set,
        GateStage::Initial,
        Some(&labels),
        None,
        Some("ADR 0017"),
    ));
    assert!(e.contains("blind set") && e.contains("heldout"), "{e}");
}

#[test]
fn the_initial_stage_refuses_a_non_blind_set_and_too_few_items() {
    let mut w = World::new("initial-refusals");
    let dev = "source-fidelity/dev-v1";
    w.sealed(dev, "dev", "dev", 7);
    let text = matrix_text(dev, 50.0);
    let eval_dir = w.run(&text, "");
    let e = err(w.gate(&eval_dir, &text, dev, GateStage::Initial, None, None, None));
    assert!(e.contains("blind set") && e.contains("dev"), "{e}");
    // An evaluation that stopped at its cap holds no result for every item: refused as such.
    let summary_path = eval_dir.join("summary.json");
    let summary = std::fs::read_to_string(&summary_path).unwrap();
    std::fs::write(
        &summary_path,
        summary.replace("\"budget_exhausted\": false", "\"budget_exhausted\": true"),
    )
    .unwrap();
    let e = err(w.gate(&eval_dir, &text, dev, GateStage::Initial, None, None, None));
    assert!(e.contains("spend cap"), "{e}");
    std::fs::write(&summary_path, summary).unwrap();

    let small = "source-fidelity/small-v1";
    let labels = w.blind(small, "small", 5);
    let text = matrix_text(small, 50.0);
    let e = err(w.gate(
        &w.dir,
        &text,
        small,
        GateStage::Initial,
        Some(&labels),
        None,
        None,
    ));
    assert!(e.contains("at least 20 items") && e.contains("15"), "{e}");
    assert!(
        !w.evalsets.join("receipts").exists(),
        "a refusal writes nothing"
    );
    assert!(!w.admissions.exists());
}

#[test]
fn the_confirmation_stage_needs_the_initial_receipt_enough_disjoint_items_and_the_same_judging() {
    let mut w = World::new("confirmation");
    let blind = "source-fidelity/blind-v1";
    let labels = w.blind(blind, "fam", 8);
    let text = matrix_text(blind, 50.0);
    let eval_dir = w.run(&text, "");
    let initial = w
        .gate(
            &eval_dir,
            &text,
            blind,
            GateStage::Initial,
            Some(&labels),
            None,
            None,
        )
        .unwrap();
    let initial_id = initial.receipt.receipt.id.clone();

    // A heldout set of 17 disjoint families: 51 items.
    let confirm = "source-fidelity/confirm-v1";
    w.sealed(confirm, "heldout", "conf", 17);
    let ctext = matrix_text(confirm, 50.0);
    let ceval = w.run(&ctext, "");
    let e = err(w.gate(
        &ceval,
        &ctext,
        confirm,
        GateStage::Confirmation,
        None,
        None,
        None,
    ));
    assert!(e.contains("--initial-receipt"), "{e}");
    let e = err(w.gate(
        &ceval,
        &ctext,
        confirm,
        GateStage::Confirmation,
        None,
        Some("nope"),
        None,
    ));
    assert!(e.contains("no receipt"), "{e}");
    let e = err(w.gate(
        &eval_dir,
        &text,
        blind,
        GateStage::Confirmation,
        Some(&labels),
        Some(&initial_id),
        None,
    ));
    assert!(e.contains("distinct"), "{e}");

    // Structural refusals come before any result is read: too few items, and a family that also
    // appears in the initial set.
    let few = "source-fidelity/few-v1";
    w.sealed(few, "heldout", "few", 10);
    let ftext = matrix_text(few, 50.0);
    let e = err(w.gate(
        &w.dir,
        &ftext,
        few,
        GateStage::Confirmation,
        None,
        Some(&initial_id),
        None,
    ));
    assert!(e.contains("at least 50 items") && e.contains("30"), "{e}");
    let overlap = "source-fidelity/overlap-v1";
    w.sealed(overlap, "heldout", "fam", 17);
    let otext = matrix_text(overlap, 50.0);
    let e = err(w.gate(
        &w.dir,
        &otext,
        overlap,
        GateStage::Confirmation,
        None,
        Some(&initial_id),
        None,
    ));
    assert!(e.contains("fam-0") && e.contains(blind), "{e}");

    // The real thing.
    let r = w
        .gate(
            &ceval,
            &ctext,
            confirm,
            GateStage::Confirmation,
            None,
            Some(&initial_id),
            None,
        )
        .unwrap();
    let b = &r.receipt.receipt;
    assert_eq!(b.outcome, Outcome::Admitted, "{:?}", b.failed);
    assert_eq!(
        (b.items, b.x.detected, b.bootstrap.differing_families),
        (51, 34, 17)
    );
    assert_eq!(b.initial_receipt.as_deref(), Some(initial_id.as_str()));
    assert_eq!(b.judging_sha256, initial.receipt.receipt.judging_sha256);
    let a = Admissions::load(&w.admissions).unwrap();
    assert_eq!(
        a.stage("fake_regions", "source-fidelity"),
        Some(Stage::Confirmed)
    );
    assert_eq!(a.admitted("source-fidelity"), ["fake_regions"]);
    assert_eq!(a.admission.len(), 1, "one record per tool and use case");
    assert_eq!(a.admission[0].receipt, b.id);
    // The heldout set's manifest is untouched: nothing to publish.
    assert!(!r.written.iter().any(|p| p.ends_with("set.toml")));

    // A new run of the confirmation set under a changed policy: scoring it again is a repeat,
    // refused without an ADR reference; under one, the judging configuration differs from the
    // initial stage's and the gate refuses before writing anything.
    let changed = w.run(&ctext, "\n[policy]\non_unsure = \"pass\"\n");
    let e = err(w.gate(
        &changed,
        &ctext,
        confirm,
        GateStage::Confirmation,
        None,
        Some(&initial_id),
        None,
    ));
    assert!(e.contains("already scored"), "{e}");
    let e = err(w.gate(
        &changed,
        &ctext,
        confirm,
        GateStage::Confirmation,
        None,
        Some(&initial_id),
        Some("ADR 0017"),
    ));
    assert!(e.contains("judging configuration differs"), "{e}");
    assert_eq!(Receipt::load_all(&w.evalsets).unwrap().len(), 2);

    // The same configuration again under the ADR reference is allowed once; the reference is then
    // spent.
    let ctext2 = matrix_text(confirm, 60.0);
    let ceval2 = w.run(&ctext2, "");
    let r2 = w
        .gate(
            &ceval2,
            &ctext2,
            confirm,
            GateStage::Confirmation,
            None,
            Some(&initial_id),
            Some("ADR 0017"),
        )
        .unwrap();
    assert_eq!(r2.receipt.receipt.adr.as_deref(), Some("ADR 0017"));
    assert_ne!(r2.receipt.receipt.id, b.id);
    let e = err(w.gate(
        &changed,
        &ctext,
        confirm,
        GateStage::Confirmation,
        None,
        Some(&initial_id),
        Some("ADR 0017"),
    ));
    assert!(e.contains("already permitted"), "{e}");
    assert_eq!(Receipt::load_all(&w.evalsets).unwrap().len(), 3);
}

#[test]
fn a_matrix_without_a_gate_section_or_the_arms_cannot_be_scored() {
    let w = World::new("no-gate");
    let text = "[matrix]\nname = \"a\"\nconfig = \"fixtures/config/minimal.toml\"\nsets = [\"source-fidelity/pairs-v1\"]\narms = [\"A\"]\ndecision_runs = 1\nrepetitions = 1\nspend_cap_usd = 5.0\n\n[[matrix.models]]\nprofile = \"fake\"\nmodel = \"fake-model\"\n";
    let e = err(w.gate(
        &w.dir,
        text,
        "source-fidelity/pairs-v1",
        GateStage::Initial,
        None,
        None,
        None,
    ));
    assert!(e.contains("[matrix.gate]"), "{e}");
    let text = text.replace(
        "spend_cap_usd = 5.0",
        "spend_cap_usd = 5.0\n\n[matrix.gate]\nseed = 1",
    );
    let e = err(w.gate(
        &w.dir,
        &text,
        "source-fidelity/pairs-v1",
        GateStage::Initial,
        None,
        None,
        None,
    ));
    assert!(e.contains("arms C and D_fake_regions"), "{e}");
    let m = Matrix::parse(&text).unwrap();
    assert_eq!(
        m.matrix.gate.as_ref().unwrap().resamples,
        10_000,
        "the default"
    );
    assert_eq!(m.hypotheses(), 1, "no tool arm still counts one hypothesis");
    let two = text.replace("arms = [\"A\"]", "arms = [\"C\", \"D_x\", \"D_y\"]");
    assert_eq!(Matrix::parse(&two).unwrap().hypotheses(), 2);
    assert_ne!(Matrix::parse(&two).unwrap().identity(), m.identity());
    assert!("maybe".parse::<GateStage>().is_err());
}

fn boot(lower: f64, differing: usize, insufficient: bool) -> Bootstrap {
    Bootstrap {
        seed: 1,
        resamples: 1000,
        hypotheses: 1,
        families: 8,
        differing_families: differing,
        point: 0.5,
        lower_percentile: 0.025,
        upper_percentile: 0.975,
        lower,
        upper: 0.9,
        insufficient_tail: insufficient,
    }
}

fn arm(
    detected: usize,
    missed: usize,
    errors: usize,
    false_rejects: usize,
    unsure: usize,
    cost: f64,
) -> ArmMetrics {
    ArmMetrics {
        items: 20,
        expected_fail: 10,
        expected_pass: 10,
        detected,
        missed,
        false_rejects,
        unsure,
        errors,
        cost_usd: cost,
    }
}

#[test]
fn every_threshold_holds_at_its_boundary_and_fails_one_past_it() {
    let t = Thresholds::default();
    let c = arm(2, 6, 0, 0, 0, 1.0);
    let ok = |x: &ArmMetrics, b: &Bootstrap| decide(&c, x, b, &t);
    // Each row: a case exactly at the boundary (admitted) and one past it (not admitted).
    let table: Vec<(
        &str,
        ArmMetrics,
        Bootstrap,
        Outcome,
        ArmMetrics,
        Bootstrap,
        Outcome,
    )> = vec![
        (
            "error rate 5%",
            arm(7, 1, 1, 0, 0, 1.0),
            boot(0.1, 5, false),
            Outcome::Rejected,
            arm(7, 1, 2, 0, 0, 1.0),
            boot(0.1, 5, false),
            Outcome::Rejected,
        ),
        (
            "detected gain 3",
            arm(5, 1, 0, 0, 0, 1.0),
            boot(0.1, 5, false),
            Outcome::Admitted,
            arm(4, 1, 0, 0, 0, 1.0),
            boot(0.1, 5, false),
            Outcome::Inconclusive,
        ),
        (
            "false rejects +1",
            arm(7, 1, 0, 1, 0, 1.0),
            boot(0.1, 5, false),
            Outcome::Admitted,
            arm(7, 1, 0, 2, 0, 1.0),
            boot(0.1, 5, false),
            Outcome::Inconclusive,
        ),
        (
            "unsure 25%",
            arm(7, 1, 0, 0, 5, 1.0),
            boot(0.1, 5, false),
            Outcome::Admitted,
            arm(7, 1, 0, 0, 6, 1.0),
            boot(0.1, 5, false),
            Outcome::Inconclusive,
        ),
        (
            "cost 2x",
            arm(7, 1, 0, 0, 0, 2.0),
            boot(0.1, 5, false),
            Outcome::Admitted,
            arm(7, 1, 0, 0, 0, 2.01),
            boot(0.1, 5, false),
            Outcome::Inconclusive,
        ),
        (
            "families 5",
            arm(7, 1, 0, 0, 0, 1.0),
            boot(0.1, 5, false),
            Outcome::Admitted,
            arm(7, 1, 0, 0, 0, 1.0),
            boot(0.1, 4, false),
            Outcome::Inconclusive,
        ),
        (
            "lower bound > 0",
            arm(7, 1, 0, 0, 0, 1.0),
            boot(1e-9, 5, false),
            Outcome::Admitted,
            arm(7, 1, 0, 0, 0, 1.0),
            boot(0.0, 5, false),
            Outcome::Inconclusive,
        ),
        (
            "tail resolved",
            arm(7, 1, 0, 0, 0, 1.0),
            boot(0.1, 5, false),
            Outcome::Admitted,
            arm(7, 1, 0, 0, 0, 1.0),
            boot(0.1, 5, true),
            Outcome::Inconclusive,
        ),
    ];
    for (name, at, b_at, want_at, past, b_past, want_past) in &table {
        assert_eq!(ok(at, b_at).outcome, *want_at, "{name}: at the boundary");
        assert_eq!(
            ok(past, b_past).outcome,
            *want_past,
            "{name}: past the boundary"
        );
    }
    // The error-rate ceiling on its own: errors in X at the ceiling with C erring as much.
    let c_err = arm(2, 6, 1, 0, 0, 1.0);
    assert_eq!(
        decide(&c_err, &arm(7, 1, 1, 0, 0, 1.0), &boot(0.1, 5, false), &t).outcome,
        Outcome::Admitted,
        "5% exactly holds"
    );
    let c_err2 = arm(2, 6, 2, 0, 0, 1.0);
    assert_eq!(
        decide(&c_err2, &arm(7, 1, 2, 0, 0, 1.0), &boot(0.1, 5, false), &t).outcome,
        Outcome::Inconclusive,
        "10% is over the ceiling, not more than C"
    );
    // Precedence: a rejection reason outranks every inconclusive one.
    let v = decide(&c, &arm(7, 7, 0, 5, 9, 9.0), &boot(0.0, 1, true), &t);
    assert_eq!(v.outcome, Outcome::Rejected);
    assert!(v.failed.len() >= 5, "{:?}", v.failed);
    // Multiplicity: two hypotheses halve the lower percentile, and 1000 resamples cannot resolve it.
    let items: Vec<squint_eval::bootstrap::PairedItem> = (0..8)
        .flat_map(|f| {
            (0..2).map(move |i| squint_eval::bootstrap::PairedItem {
                family: format!("f{f}"),
                id: format!("f{f}-{i}"),
                expected_fail: true,
                detected_c: false,
                detected_x: true,
            })
        })
        .collect();
    let one = squint_eval::bootstrap::paired_bootstrap(&items, 1, 1000, 1).unwrap();
    let two = squint_eval::bootstrap::paired_bootstrap(&items, 1, 1000, 2).unwrap();
    assert!(
        (two.lower_percentile - 0.0125).abs() < 1e-12
            && !one.insufficient_tail
            && two.insufficient_tail
    );
    assert!(
        !squint_eval::bootstrap::paired_bootstrap(&items, 1, 1600, 2)
            .unwrap()
            .insufficient_tail
    );
    let _ = Policy {
        on_unsure: PolicyOutcome::Review,
        on_error: PolicyOutcome::Review,
    };
}
