//! Why the per-call cap refused a check (task 11.4): the decision envelope broken down from the
//! report, the price tier it was bounded at and why, that it is a worst case and not an expected
//! cost, the settings that lower it or raise the cap with their values now, and, when the output
//! directory holds any, what recent completed checks of the same model and use case really spent.
//! Nothing here estimates a typical cost.

use std::fmt::Write as _;
use std::path::Path;

use squint_core::config::Config;
use squint_core::ledger::{Envelope, PlannedRequest};
use squint_core::report::Report;

/// How many recent decisions' spend to show, and how many directories to look into for them.
const RECENT_SHOWN: usize = 5;
const RECENT_SCANNED: usize = 200;

/// The explanation, when a run of the decision ended in `cap` and the envelope was planned.
pub fn cap_refusal(
    report: &Report,
    config: &Config,
    configured_cap: f64,
    out_dir: &Path,
    this: &Path,
) -> Option<String> {
    let capped = report
        .runs
        .iter()
        .any(|r| r.error.as_ref().is_some_and(|e| e.class == "cap"));
    let envelope = report.cost.envelope.as_ref()?;
    if !capped {
        return None;
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "cap: the envelope (${:.4}) is the most this check could spend, reserved against the \
         ${:.2} cap before the first request; it is a worst-case bound, not an expected cost.",
        envelope.usd, report.cost.cap_usd
    );
    if report.cost.attempts == 0 {
        let _ = writeln!(
            out,
            "  The check was refused before any request: nothing was sent."
        );
    } else {
        let _ = writeln!(
            out,
            "  After {} request attempts (${:.4}), a retry was refused: the envelope's balance \
             could no longer cover its bound.",
            report.cost.attempts, report.cost.spent_usd
        );
    }
    out.push_str(&breakdown(envelope));
    if let Some(line) = tier_line(config, envelope) {
        let _ = writeln!(out, "  {line}");
    }
    let model = &config.provider.model;
    let recent = recent_spend(out_dir, this, model, &report.use_case);
    if !recent.is_empty() {
        let amounts: Vec<String> = recent.iter().map(|usd| format!("${usd:.4}")).collect();
        let _ = writeln!(
            out,
            "  actually spent by recent completed {} checks on {model} in {} (newest first): {}",
            report.use_case,
            out_dir.display(),
            amounts.join(", ")
        );
    }
    out.push_str(&knobs(config, configured_cap));
    Some(out.trim_end().to_string())
}

/// The requests one run plans, grouped by kind in order, the retry reserve and the runs.
fn breakdown(envelope: &Envelope) -> String {
    let mut out = String::new();
    let mut groups: Vec<(&str, Vec<&PlannedRequest>)> = Vec::new();
    for r in &envelope.requests {
        match groups.last_mut() {
            Some((kind, list)) if *kind == r.kind.as_str() => list.push(r),
            _ => groups.push((r.kind.as_str(), vec![r])),
        }
    }
    let per_run: f64 =
        envelope.requests.iter().map(|r| r.bound_usd).sum::<f64>() + envelope.retry_reserve_usd;
    let _ = writeln!(
        out,
        "  each run plans, at full output and with every earlier turn in its history:"
    );
    for (kind, list) in &groups {
        let total: f64 = list.iter().map(|r| r.bound_usd).sum();
        let outputs: Vec<u64> = list.iter().map(|r| r.shape.output_tokens).collect();
        let output = if outputs.iter().all(|o| *o == outputs[0]) {
            let each = if outputs.len() > 1 { " each" } else { "" };
            format!("{} output tokens{each}", outputs[0])
        } else {
            format!(
                "{}–{} output tokens",
                outputs.iter().min().unwrap_or(&0),
                outputs.iter().max().unwrap_or(&0)
            )
        };
        let inputs: Vec<u64> = list.iter().map(|r| r.shape.input_tokens).collect();
        let _ = writeln!(
            out,
            "    {} × {kind:<10} up to {output}, {} input tokens{}: ${total:.4}",
            list.len(),
            span(&inputs),
            if list.len() > 1 { " (growing)" } else { "" },
        );
    }
    let largest = envelope
        .requests
        .iter()
        .map(|r| r.bound_usd)
        .fold(0.0, f64::max);
    let retries = if largest > 0.0 {
        (envelope.retry_reserve_usd / largest).round()
    } else {
        0.0
    };
    let _ = writeln!(
        out,
        "    retry reserve: {retries} retries × the largest request (${largest:.4}): ${:.4}",
        envelope.retry_reserve_usd
    );
    let _ = writeln!(
        out,
        "  ${per_run:.4} a run × {} decision run{} = ${:.4}",
        envelope.decision_runs,
        if envelope.decision_runs == 1 { "" } else { "s" },
        envelope.usd
    );
    out
}

/// `12000` or `12000–48000`.
fn span(values: &[u64]) -> String {
    match (values.iter().min(), values.iter().max()) {
        (Some(a), Some(b)) if a == b => a.to_string(),
        (Some(a), Some(b)) => format!("{a}–{b}"),
        _ => "0".into(),
    }
}

/// Which price tier the bounds were taken at, and why, from the catalog's entry for the model.
fn tier_line(config: &Config, envelope: &Envelope) -> Option<String> {
    let catalog = squint_providers::catalog::Catalog::shipped().ok()?;
    let model_id = &config.provider.model;
    let Some(model) = catalog.model(model_id) else {
        return Some(format!(
            "prices: {model_id} is not in the catalog (a scripted provider), so its own single \
             price list applies"
        ));
    };
    let tiers: Vec<_> = envelope
        .requests
        .iter()
        .map(|r| model.bound_tier(&r.shape))
        .collect();
    let long = tiers.iter().filter(|t| t.long_prompt).count();
    let first = tiers.first()?;
    Some(match first.long_prompt_threshold {
        None => format!(
            "prices: {model_id} has one price tier (${}/M input, ${}/M output tokens)",
            first.input_per_million, first.output_per_million
        ),
        Some(threshold) if long > 0 => {
            let lp = tiers.iter().find(|t| t.long_prompt).unwrap_or(first);
            format!(
                "prices: {long} of {} requests {} bounded at {model_id}'s long-prompt tier \
                 (${}/M input, ${}/M output tokens, against ${}/M and ${}/M below it): their \
                 planned prompts, with images at their maximum token cost, pass its {threshold}-token \
                 threshold",
                tiers.len(),
                if long == 1 { "is" } else { "are" },
                lp.input_per_million,
                lp.output_per_million,
                model.prices.input_per_million,
                model.prices.output_per_million
            )
        }
        Some(threshold) => format!(
            "prices: every request stays under {model_id}'s {threshold}-token long-prompt \
             threshold, so the base tier applies (${}/M input, ${}/M output tokens)",
            first.input_per_million, first.output_per_million
        ),
    })
}

/// The settings that move the envelope or the cap, with their values now.
fn knobs(config: &Config, configured_cap: f64) -> String {
    let b = &config.budget;
    format!(
        "  to fit: raise the cap with --max-spend-usd (up to the configured ${configured_cap:.2}) \
         or [budget] spend_cap_usd;\n\
         \x20   or shrink the envelope: --set budget.tool_turns=N (now {}), \
         --set provider.max_output_tokens=N (now {}),\n\
         \x20   --set budget.repairs=N (now {}) or budget.repair_output_tokens=N (now {}), \
         --set budget.retries=N (now {}), budget.decision_runs (now {})\n",
        b.tool_turns,
        config.provider.max_output_tokens,
        b.repairs,
        b.repair_output_tokens,
        b.retries,
        b.decision_runs,
    )
}

/// The settled spend of the most recent completed decisions under `out_dir` with this model and
/// use case (not cache hits, not `this`), newest first: real spend, never an estimate.
fn recent_spend(out_dir: &Path, this: &Path, model: &str, use_case: &str) -> Vec<f64> {
    let Ok(entries) = std::fs::read_dir(out_dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("decision-"))
                && p.file_name() != this.file_name()
        })
        .collect();
    // `decision-<unix ms>-<pid>`: newest first by the time in the name.
    let started = |p: &std::path::PathBuf| -> u64 {
        p.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.split('-').nth(1))
            .and_then(|ms| ms.parse().ok())
            .unwrap_or(0)
    };
    dirs.sort_by_key(|p| std::cmp::Reverse(started(p)));
    dirs.iter()
        .take(RECENT_SCANNED)
        .filter_map(|d| Report::read(d).ok())
        .filter(|r| {
            r.verdict.is_some()
                && !r.cache.is_hit()
                && r.use_case == use_case
                && r.config["provider"]["model"].as_str() == Some(model)
        })
        .take(RECENT_SHOWN)
        // From +0.0, so an empty spend never prints as -0.
        .map(|r| r.cost.spent_usd + 0.0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_collapse_when_equal() {
        assert_eq!(span(&[5, 5]), "5");
        assert_eq!(span(&[9, 3, 5]), "3–9");
    }
}
