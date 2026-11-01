//! Task 3.4a: the durable evaluation ledger. Every decision's reservation is persisted before
//! anything is sent, settlement happens at most once, a reservation left unsettled by a crash is
//! charged at its bound on recovery, and cumulative spend across restarts never exceeds the cap.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::ledger::Ledger;
use squint_eval::eval_ledger::{EvalLedger, Fault};

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-eval-ledger-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn reservations_and_settlements_persist_and_recover() {
    let dir = tempdir("basic");
    let ledger = EvalLedger::open(&dir, 10.0).unwrap();
    ledger.begin("item-1/cell-a/1");
    ledger.reserve(3.0).unwrap();
    assert_eq!(ledger.reserved(), 3.0);
    ledger.settle(3.0, 0.4);
    assert_eq!(ledger.spent(), 0.4);
    assert_eq!(ledger.reserved(), 0.0);
    // A second decision in flight when the process dies: recovered as charged at its bound.
    ledger.begin("item-2/cell-a/1");
    ledger.reserve(2.5).unwrap();
    drop(ledger);
    let again = EvalLedger::open(&dir, 10.0).unwrap();
    assert_eq!(again.recovered(), ["item-2/cell-a/1"]);
    assert!(
        (again.spent() - 2.9).abs() < 1e-9,
        "0.4 settled plus 2.5 charged at bound"
    );
    assert_eq!(again.reserved(), 0.0);
    assert_eq!(again.settled(), ["item-1/cell-a/1"]);
    // Settlement is at most once: a late settle for the recovered decision changes nothing.
    again.begin("item-2/cell-a/1");
    again.settle(2.5, 0.1);
    assert!((again.spent() - 2.9).abs() < 1e-9);
}

#[test]
fn admission_is_against_the_durable_remaining_cap() {
    let dir = tempdir("cap");
    let ledger = EvalLedger::open(&dir, 5.0).unwrap();
    ledger.begin("a");
    ledger.reserve(3.0).unwrap();
    ledger.begin("b");
    let err = ledger.reserve(2.5).unwrap_err();
    assert_eq!(err.class(), "budget");
    assert!(err.to_string().contains("remaining"), "{err}");
    ledger.begin("a");
    ledger.settle(3.0, 1.0);
    ledger.begin("b");
    ledger.reserve(2.5).unwrap();
    ledger.settle(2.5, 2.5);
    drop(ledger);
    // Restart: 1.0 + 2.5 spent, 1.5 left; a 2.0 reservation is refused, a 1.5 one fits.
    let again = EvalLedger::open(&dir, 5.0).unwrap();
    assert!((again.spent() - 3.5).abs() < 1e-9);
    again.begin("c");
    assert!(again.reserve(2.0).is_err());
    again.reserve(1.5).unwrap();
}

#[test]
fn a_crash_at_every_fault_point_recovers_to_a_consistent_ledger() {
    for (fault, expect_spent, expect_recovered) in [
        (Fault::BeforeReservePersisted, 0.0, 0),
        (Fault::AfterReservePersisted, 2.0, 1),
        (Fault::BeforeSettlePersisted, 2.0, 1),
        (Fault::AfterSettlePersisted, 0.3, 0),
    ] {
        let dir = tempdir(&format!("fault-{fault:?}"));
        let ledger = EvalLedger::open(&dir, 10.0).unwrap();
        ledger.begin("d");
        ledger.inject(Some(fault));
        let reserve = ledger.reserve(2.0);
        if fault == Fault::BeforeReservePersisted || fault == Fault::AfterReservePersisted {
            assert!(
                reserve.is_err(),
                "{fault:?}: the crash surfaces as an error"
            );
        } else {
            reserve.unwrap();
            ledger.settle(2.0, 0.3);
        }
        drop(ledger);
        let again = EvalLedger::open(&dir, 10.0).unwrap();
        assert!(
            (again.spent() - expect_spent).abs() < 1e-9,
            "{fault:?}: spent {}",
            again.spent()
        );
        assert_eq!(again.recovered().len(), expect_recovered, "{fault:?}");
        assert!(again.spent() + again.reserved() <= 10.0 + 1e-9);
    }
}

#[test]
fn cumulative_spend_across_restarts_never_exceeds_the_cap() {
    let dir = tempdir("cumulative");
    let cap = 4.0;
    let mut admitted = 0;
    let mut refused = 0;
    for round in 0..12 {
        let ledger = EvalLedger::open(&dir, cap).unwrap();
        for k in 0..3 {
            let id = format!("r{round}-d{k}");
            ledger.begin(&id);
            match ledger.reserve(0.7) {
                Ok(()) => {
                    admitted += 1;
                    // Every third decision is cut off before settlement: charged at bound.
                    if k != 2 {
                        ledger.settle(0.7, 0.2);
                    }
                }
                Err(_) => refused += 1,
            }
        }
        assert!(
            ledger.spent() + ledger.reserved() <= cap + 1e-9,
            "round {round}"
        );
        drop(ledger);
    }
    let final_ledger = EvalLedger::open(&dir, cap).unwrap();
    assert!(final_ledger.spent() <= cap + 1e-9);
    assert!(
        admitted > 0 && refused > 0,
        "admitted {admitted}, refused {refused}"
    );
}

#[test]
fn a_decision_run_again_after_a_crash_is_charged_for_both_attempts() {
    let dir = tempdir("attempts");
    let ledger = EvalLedger::open(&dir, 10.0).unwrap();
    ledger.begin("item-1/cell-a/1");
    ledger.reserve(2.0).unwrap();
    // The process dies with the attempt in flight: on reopen it is charged at its bound.
    drop(ledger);
    let again = EvalLedger::open(&dir, 10.0).unwrap();
    assert!((again.spent() - 2.0).abs() < 1e-9);
    assert_eq!(again.recovered(), ["item-1/cell-a/1"]);
    // The runner redoes the same decision: a second attempt, with its own hold and settlement,
    // on top of the charged first one.
    again.begin("item-1/cell-a/1");
    again.reserve(2.0).unwrap();
    assert!((again.reserved() - 2.0).abs() < 1e-9);
    again.settle(2.0, 0.3);
    assert!(
        (again.spent() - 2.3).abs() < 1e-9,
        "the crashed attempt at bound plus the re-run as settled"
    );
    assert_eq!(again.reserved(), 0.0);
    // A late settlement of the same attempt changes nothing; and the file replays to the same.
    again.settle(2.0, 0.3);
    assert!((again.spent() - 2.3).abs() < 1e-9);
    drop(again);
    let third = EvalLedger::open(&dir, 10.0).unwrap();
    assert!(
        (third.spent() - 2.3).abs() < 1e-9,
        "replay keeps both attempts"
    );
    assert_eq!(
        third.recovered(),
        ["item-1/cell-a/1"],
        "the crashed attempt stays charged"
    );
    assert_eq!(third.settled(), ["item-1/cell-a/1"]);
    // A crash between the settlement line and the result file is the same story: the next
    // attempt is a fresh hold, never a free re-run.
    third.begin("item-1/cell-a/1");
    third.reserve(1.0).unwrap();
    third.settle(1.0, 0.1);
    assert!((third.spent() - 2.4).abs() < 1e-9);
}
