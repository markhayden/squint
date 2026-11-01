//! Cost ledger (SPEC "Cost ledger"). The unit of reservation is the **decision envelope**: the
//! upper bound of every request the decision can make, each counted once per possible attempt, at
//! catalog prices. It is reserved once against the per-job cap (and any shared evaluation ledger)
//! before the first request. Each billable attempt moves its bound from the envelope's unallocated
//! balance into a hold; a reported usage settles the hold to actual cost and returns the
//! difference; unknown usage charges the bound. The balance is released when the decision ends.
//!
//! Prices and token estimates come from the adapter's catalog entry through `Pricing`; until the
//! catalog (task 2.1) a linear `Prices` list stands in. Bounds are estimates: when a request
//! costs more than its planned bound the account records the overrun rather than failing the run,
//! because the cap was enforced at reservation.

use std::sync::Mutex;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::provider::Usage;
use crate::{Error, Result};

/// The shape of a request for bounding its cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RequestShape {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub images: u64,
}

/// What the ledger needs from the model's catalog entry.
pub trait Pricing: Send + Sync {
    /// The upper-bound cost in USD of a request of this shape, or an error when it cannot be
    /// bounded.
    fn bound(&self, shape: &RequestShape) -> Result<f64>;
    /// The cost in USD of reported usage.
    fn cost(&self, usage: &Usage) -> f64;
}

/// A linear price list: the stub catalog entry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Prices {
    pub input_per_million: f64,
    pub output_per_million: f64,
    /// Charged per image on the bound only; adapters that bill images as input tokens fold them
    /// into `input_tokens` instead.
    pub per_image: f64,
}

impl Pricing for Prices {
    fn bound(&self, shape: &RequestShape) -> Result<f64> {
        Ok(shape.input_tokens as f64 * self.input_per_million / 1e6
            + shape.output_tokens as f64 * self.output_per_million / 1e6
            + shape.images as f64 * self.per_image)
    }

    fn cost(&self, usage: &Usage) -> f64 {
        usage.input_tokens as f64 * self.input_per_million / 1e6
            + usage.output_tokens as f64 * self.output_per_million / 1e6
    }
}

/// Conservative tokens for planned text: one token per three bytes.
pub fn planned_tokens(bytes: u64) -> u64 {
    bytes.div_ceil(3)
}

/// Typical tokens for text as sent: one token per four bytes.
pub fn estimated_tokens(bytes: u64) -> u64 {
    bytes.div_ceil(4)
}

/// What the plan needs to know about the resolved run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanInput {
    pub system_bytes: u64,
    pub tools_bytes: u64,
    pub first_look_bytes: u64,
    pub checklist_bytes: u64,
    pub job_images: u64,
    /// The most text one tool result can put before the model (`Bounds::presented_bytes`), and
    /// the most images; zero without tools.
    pub tool_envelope_bytes: u64,
    pub tool_images: u64,
}

/// One planned request of the envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlannedRequest {
    pub kind: String,
    pub shape: RequestShape,
    /// The bound of one attempt.
    pub bound_usd: f64,
}

/// The decision envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub usd: f64,
    pub decision_runs: u32,
    /// One run's requests, in order, each planned once; every run is planned the same.
    pub requests: Vec<PlannedRequest>,
    /// Per run, what transport retries may draw: `retries` times the largest request bound.
    pub retry_reserve_usd: f64,
    /// Billable attempts the envelope can pay for across the decision: every planned request
    /// once, plus `retries` per run.
    pub attempts: u32,
}

/// Bytes added by a checklist line per evidence item.
const EVIDENCE_LINE_BYTES: u64 = 160;
/// Framing a request carries beyond its content (instructions, repair notices, labels).
const FRAMING_BYTES: u64 = 4096;
/// Headroom on every planned input, since byte-based token estimates are not exact.
const HEADROOM: f64 = 1.1;

/// Plans the decision envelope: per run, every tool turn, the final turn and every repair the
/// budget allows, each bounded by the history it could carry and planned once, plus a retry
/// reserve of `retries` times the largest bound. A retryable failure is charged at its bound, so
/// the reserve pays for `retries` failures anywhere in the run; a retry it cannot cover is refused
/// (`error: budget`) rather than made.
pub fn plan(config: &Config, input: &PlanInput, pricing: &dyn Pricing) -> Result<Envelope> {
    let b = &config.budget;
    let base_bytes =
        input.system_bytes + input.tools_bytes + input.first_look_bytes + FRAMING_BYTES;
    let checklist_bytes = input.checklist_bytes + u64::from(b.evidence_items) * EVIDENCE_LINE_BYTES;
    // The whole call budget may be spent at any point, so every request after the first carries
    // every tool result the run can produce.
    let tool_tokens = planned_tokens(u64::from(b.tool_calls) * input.tool_envelope_bytes);
    let tool_images = u64::from(b.tool_calls) * input.tool_images;
    let max_output = u64::from(config.provider.max_output_tokens);

    let mut requests = Vec::new();
    let total = b.tool_turns + 1 + b.repairs;
    for k in 1..=total {
        let prior = u64::from(k - 1);
        let kind = if k <= b.tool_turns {
            "tool_turn"
        } else if k == b.tool_turns + 1 {
            "final_turn"
        } else {
            "repair"
        };
        let mut input_tokens = planned_tokens(base_bytes)
            + prior * (max_output + planned_tokens(checklist_bytes + FRAMING_BYTES));
        let mut images = input.job_images;
        if prior > 0 {
            input_tokens += tool_tokens;
            images += tool_images;
        }
        if kind != "tool_turn" {
            // The final instruction repeats the checklist.
            input_tokens += planned_tokens(checklist_bytes);
        }
        let input_tokens = (input_tokens as f64 * HEADROOM).ceil() as u64;
        let output_tokens = if kind == "repair" {
            u64::from(b.repair_output_tokens)
        } else {
            max_output
        };
        let shape = RequestShape {
            input_tokens,
            output_tokens,
            images,
        };
        let bound_usd = pricing.bound(&shape)?;
        if !bound_usd.is_finite() || bound_usd < 0.0 {
            return Err(Error::Budget(format!(
                "the bound of a {kind} request is not a finite non-negative amount ({bound_usd})"
            )));
        }
        requests.push(PlannedRequest {
            kind: kind.into(),
            shape,
            bound_usd,
        });
    }
    let largest = requests.iter().map(|r| r.bound_usd).fold(0.0, f64::max);
    let retry_reserve_usd = f64::from(b.retries) * largest;
    let per_run: f64 = requests.iter().map(|r| r.bound_usd).sum::<f64>() + retry_reserve_usd;
    Ok(Envelope {
        usd: per_run * f64::from(b.decision_runs),
        decision_runs: b.decision_runs,
        attempts: (total + b.retries) * b.decision_runs,
        retry_reserve_usd,
        requests,
    })
}

/// A cap shared across concurrent decisions. `reserve` is atomic with respect to other
/// reservations; `settle` moves a reservation to spend and releases the rest.
pub trait Ledger: Send + Sync {
    fn cap(&self) -> f64;
    fn reserve(&self, usd: f64) -> Result<()>;
    fn settle(&self, reserved_usd: f64, spent_usd: f64);
    fn reserved(&self) -> f64;
    fn spent(&self) -> f64;
    /// [`Ledger::settle`] with the part of `spent_usd` no provider answer settled (table-manners
    /// 2.3), in one step so an observer never sees one without the other. Ledgers that do not
    /// report unresolved spend settle as usual.
    fn settle_with_unresolved(&self, reserved_usd: f64, spent_usd: f64, _unresolved_usd: f64) {
        self.settle(reserved_usd, spent_usd);
    }
}

/// The per-job ledger: one cap, a mutex around the balances.
#[derive(Debug)]
pub struct CapLedger {
    cap: f64,
    balances: Mutex<Balances>,
}

#[derive(Debug, Default, Clone, Copy)]
struct Balances {
    reserved: f64,
    spent: f64,
    unresolved: f64,
}

/// Rounding slack on currency comparisons.
const EPSILON: f64 = 1e-9;

impl CapLedger {
    pub fn new(cap: f64) -> CapLedger {
        CapLedger {
            cap,
            balances: Mutex::new(Balances::default()),
        }
    }

    fn balances(&self) -> Balances {
        self.balances.lock().map(|b| *b).unwrap_or_default()
    }

    /// Spent, reserved and unresolved read together, so an observer never sees a reservation
    /// settled into spend on one side and not the other.
    pub fn snapshot(&self) -> (f64, f64, f64) {
        let b = self.balances();
        (b.spent, b.reserved, b.unresolved)
    }
}

impl Ledger for CapLedger {
    fn cap(&self) -> f64 {
        self.cap
    }

    fn reserve(&self, usd: f64) -> Result<()> {
        let mut b = self
            .balances
            .lock()
            .map_err(|_| Error::Budget("ledger poisoned".into()))?;
        let remaining = self.cap - b.reserved - b.spent;
        if usd > remaining + EPSILON {
            return Err(Error::Budget(format!(
                "decision envelope ${usd:.4} exceeds the remaining ${remaining:.4} of the ${:.2} cap",
                self.cap
            )));
        }
        b.reserved += usd;
        Ok(())
    }

    fn settle(&self, reserved_usd: f64, spent_usd: f64) {
        if let Ok(mut b) = self.balances.lock() {
            b.reserved = (b.reserved - reserved_usd).max(0.0);
            b.spent += spent_usd;
        }
    }

    fn reserved(&self) -> f64 {
        self.balances().reserved
    }

    fn spent(&self) -> f64 {
        self.balances().spent
    }

    fn settle_with_unresolved(&self, reserved_usd: f64, spent_usd: f64, unresolved_usd: f64) {
        if let Ok(mut b) = self.balances.lock() {
            b.reserved = (b.reserved - reserved_usd).max(0.0);
            b.spent += spent_usd;
            b.unresolved += unresolved_usd.max(0.0);
        }
    }
}

/// A hold on part of the envelope for one billable attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hold(usize);

/// One decision's envelope accounting.
#[derive(Debug, Clone)]
pub struct Account {
    envelope_usd: f64,
    unallocated: f64,
    holds: Vec<Option<f64>>,
    settled: f64,
    /// The part of `settled` charged at a bound because no answer settled it.
    unresolved: f64,
    overrun: f64,
}

/// The account at the end of the decision.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Closing {
    pub envelope_usd: f64,
    pub spent_usd: f64,
    pub released_usd: f64,
    /// Spend beyond what the envelope planned (bounds are estimates).
    pub overrun_usd: f64,
    /// The part of `spent_usd` charged at a bound because no answer settled it.
    #[serde(default)]
    pub unresolved_usd: f64,
}

impl Account {
    pub fn new(envelope_usd: f64) -> Account {
        Account {
            envelope_usd,
            unallocated: envelope_usd,
            holds: Vec::new(),
            settled: 0.0,
            unresolved: 0.0,
            overrun: 0.0,
        }
    }

    pub fn envelope_usd(&self) -> f64 {
        self.envelope_usd
    }

    /// Money not yet held or spent: what another attempt may draw.
    pub fn unallocated(&self) -> f64 {
        self.unallocated
    }

    /// Whether the balance covers one more attempt at this bound. A retry that it does not cover
    /// is refused rather than made, so a run of failures ends in `error: budget` instead of an
    /// overrun; the planned requests themselves are always made (a bound is an estimate).
    pub fn covers(&self, bound: f64) -> bool {
        self.unallocated + EPSILON >= bound
    }

    /// Money still held for the decision: the unallocated balance plus open holds.
    pub fn reserved(&self) -> f64 {
        self.unallocated + self.holds.iter().flatten().sum::<f64>()
    }

    pub fn spent(&self) -> f64 {
        self.settled
    }

    /// Moves `bound` from the unallocated balance into a hold. A bound beyond the balance takes
    /// what is left and records the rest as overrun.
    pub fn hold(&mut self, bound: f64) -> Hold {
        let bound = bound.max(0.0);
        let taken = bound.min(self.unallocated);
        self.overrun += bound - taken;
        self.unallocated -= taken;
        self.holds.push(Some(taken));
        Hold(self.holds.len() - 1)
    }

    /// Settles a hold to actual cost; returns the amount given back to the balance.
    pub fn settle(&mut self, hold: Hold, cost: f64) -> f64 {
        let Some(held) = self.holds.get_mut(hold.0).and_then(Option::take) else {
            return 0.0;
        };
        let cost = cost.max(0.0);
        self.settled += cost;
        if cost <= held {
            let back = held - cost;
            self.unallocated += back;
            back
        } else {
            let excess = cost - held;
            let covered = excess.min(self.unallocated);
            self.unallocated -= covered;
            self.overrun += excess - covered;
            0.0
        }
    }

    /// Charges a hold at its bound: the usage is unknown.
    pub fn charge(&mut self, hold: Hold) {
        if let Some(held) = self.holds.get_mut(hold.0).and_then(Option::take) {
            self.settled += held;
            self.unresolved += held;
        }
    }

    /// Closes the account: open holds are charged at their bound, the balance is released.
    pub fn close(mut self) -> Closing {
        for h in 0..self.holds.len() {
            self.charge(Hold(h));
        }
        Closing {
            envelope_usd: self.envelope_usd,
            spent_usd: self.settled,
            released_usd: self.unallocated,
            overrun_usd: self.overrun,
            unresolved_usd: self.unresolved,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close_to(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn ledger_account_covers_a_retry_only_while_the_balance_holds_its_bound() {
        // Envelope 1.0: three charged failures at 0.3 leave 0.1, which covers no fourth attempt.
        let mut a = Account::new(1.0);
        for _ in 0..3 {
            assert!(a.covers(0.3));
            let h = a.hold(0.3);
            a.charge(h);
        }
        assert!(close_to(a.unallocated(), 0.1));
        assert!(!a.covers(0.3));
        assert!(a.covers(0.1), "an exact fit is covered");
        // Settling a hold below its bound gives the balance back and covers again.
        let h = a.hold(0.1);
        a.settle(h, 0.0);
        assert!(close_to(a.unallocated(), 0.1));
        assert!(close_to(a.close().overrun_usd, 0.0));
    }

    #[test]
    fn ledger_account_table() {
        // (operation, expected reserved after, expected spent after)
        let mut a = Account::new(1.0);
        assert!(close_to(a.reserved(), 1.0));
        let h1 = a.hold(0.3);
        assert!(
            close_to(a.reserved(), 1.0),
            "a hold moves money, it does not add any"
        );
        let back = a.settle(h1, 0.1);
        assert!(close_to(back, 0.2), "settlement returns the difference");
        assert!(close_to(a.reserved(), 0.9) && close_to(a.spent(), 0.1));
        let h2 = a.hold(0.4);
        a.charge(h2);
        assert!(close_to(a.spent(), 0.5), "unknown usage charges the bound");
        assert!(close_to(a.reserved(), 0.5));
        let h3 = a.hold(0.2);
        let c = a.clone().close();
        assert!(
            close_to(c.spent_usd, 0.7),
            "an open hold is charged at close"
        );
        assert!(close_to(c.released_usd, 0.3));
        a.settle(h3, 0.0);
        let c = a.close();
        assert!(close_to(c.spent_usd, 0.5) && close_to(c.released_usd, 0.5));
        assert!(close_to(c.overrun_usd, 0.0));
    }

    #[test]
    fn ledger_reserved_plus_spent_never_exceeds_the_envelope() {
        let mut a = Account::new(2.0);
        let mut last = a.reserved();
        let ops: [(f64, Option<f64>); 6] = [
            (0.5, Some(0.2)),
            (0.7, None),
            (0.4, Some(0.4)),
            (0.3, Some(0.0)),
            (0.9, Some(0.1)),
            (0.1, None),
        ];
        for (bound, cost) in ops {
            let h = a.hold(bound);
            match cost {
                Some(c) => {
                    a.settle(h, c);
                }
                None => a.charge(h),
            }
            assert!(a.reserved() <= last + 1e-9, "reserved never increases");
            assert!(a.reserved() + a.spent() <= 2.0 + 1e-9);
            last = a.reserved();
        }
    }

    #[test]
    fn ledger_records_an_overrun_instead_of_failing() {
        let mut a = Account::new(0.1);
        let h = a.hold(0.3);
        assert!(close_to(a.reserved(), 0.1));
        a.settle(h, 0.25);
        let c = a.close();
        assert!(close_to(c.spent_usd, 0.25));
        assert!(c.overrun_usd > 0.0);
    }

    #[test]
    fn ledger_cap_refuses_what_does_not_fit_and_releases_on_settle() {
        let l = CapLedger::new(1.0);
        l.reserve(0.6).expect("fits");
        assert!(l.reserve(0.5).is_err(), "would exceed the cap");
        l.settle(0.6, 0.1);
        assert!(close_to(l.reserved(), 0.0) && close_to(l.spent(), 0.1));
        l.reserve(0.9)
            .expect("the released money is available again");
        assert!(l.reserve(0.01).is_err());
    }

    #[test]
    fn ledger_two_concurrent_decisions_cannot_overshoot_a_shared_cap() {
        use std::sync::{Arc, Barrier};
        let ledger = Arc::new(CapLedger::new(1.0));
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let ledger = Arc::clone(&ledger);
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    ledger.reserve(0.6).is_ok()
                })
            })
            .collect();
        let successes = handles
            .into_iter()
            .map(|h| h.join().expect("thread"))
            .filter(|ok| *ok)
            .count();
        assert_eq!(
            successes, 1,
            "exactly one of two 0.6 reservations fits a 1.0 cap"
        );
        assert!(close_to(ledger.reserved(), 0.6));
    }

    #[test]
    fn ledger_plan_grows_with_history_and_counts_every_attempt() {
        let text = crate::fixture("config/minimal.toml");
        let config = Config::parse(&text, &[], &|_| None).expect("config");
        let prices = Prices {
            input_per_million: 1.0,
            output_per_million: 4.0,
            per_image: 0.001,
        };
        let input = PlanInput {
            system_bytes: 6000,
            tools_bytes: 1000,
            first_look_bytes: 800,
            checklist_bytes: 300,
            job_images: 2,
            tool_envelope_bytes: 0,
            tool_images: 0,
        };
        let e = plan(&config, &input, &prices).expect("plan");
        assert_eq!(
            e.requests.len() as u32,
            config.budget.tool_turns + 1 + config.budget.repairs
        );
        for w in e.requests.windows(2) {
            assert!(
                w[1].shape.input_tokens > w[0].shape.input_tokens,
                "history grows"
            );
        }
        let repair = e.requests.last().expect("repair");
        assert_eq!(repair.kind, "repair");
        assert_eq!(
            repair.shape.output_tokens,
            u64::from(config.budget.repair_output_tokens)
        );
        assert_eq!(
            e.attempts,
            (config.budget.tool_turns + 1 + config.budget.repairs + config.budget.retries)
                * config.budget.decision_runs
        );
        // Every request once, plus `retries` times the largest bound (the repair's).
        let largest = e.requests.iter().map(|r| r.bound_usd).fold(0.0, f64::max);
        assert!(close_to(largest, repair.bound_usd));
        assert!(close_to(
            e.retry_reserve_usd,
            f64::from(config.budget.retries) * largest
        ));
        let once: f64 = e.requests.iter().map(|r| r.bound_usd).sum();
        assert!(close_to(
            e.usd,
            (once + e.retry_reserve_usd) * f64::from(config.budget.decision_runs)
        ));
    }
}
