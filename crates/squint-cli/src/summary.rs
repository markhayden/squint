//! The human summary `squint check` prints on stderr after the `decision:` line (task 11.1): each
//! checklist item with its rule and the model's reason, the findings with their locations, why
//! the verdict is what it is, the spend against the cap and the command that opens the report.
//! Everything comes from the decision's `report.json`, so a cache hit summarises the decision it
//! copies. stdout keeps the verdict word alone (the machine's answer); `--quiet` drops this.

use std::fmt::Write as _;
use std::path::Path;

use squint_core::aggregate::{Confidence, Disposition, Row, Verdict};
use squint_core::frames::{Location, LocationKind};
use squint_core::report::{FindingEntry, ItemRow, Report, RootLocation, RunReport};

/// The longest rule label and reason, in characters, before they are cut with an ellipsis.
const LABEL_CHARS: usize = 48;
const REASON_CHARS: usize = 140;

/// The summary of a decision, one line per entry, without a trailing newline.
pub fn render(report: &Report, dir: &Path) -> String {
    let mut out = String::new();
    let several = report.runs.len() > 1;
    for run in &report.runs {
        if several {
            let _ = writeln!(
                out,
                "run {}: {}",
                run.run,
                run.verdict.map(verdict_word).unwrap_or("error")
            );
        }
        if !run.items.is_empty() {
            let _ = writeln!(out, "items:");
            let width = run
                .items
                .iter()
                .map(|i| display_id(&i.id).chars().count())
                .max()
                .unwrap_or(0);
            for item in &run.items {
                let _ = writeln!(out, "  {}", item_line(run, item, width));
            }
        }
        if !run.findings.is_empty() {
            let _ = writeln!(out, "findings:");
            for f in &run.findings {
                let _ = writeln!(out, "  {}", finding_line(f));
            }
        }
    }
    let _ = writeln!(out, "why: {}", why(report));
    let _ = writeln!(out, "{}", spend(report));
    let _ = write!(out, "view: squint report view {}", dir.display());
    out
}

fn verdict_word(v: Verdict) -> &'static str {
    match v {
        Verdict::Pass => "pass",
        Verdict::Fail => "fail",
        Verdict::Unsure => "unsure",
    }
}

/// `chk:ob:RC1@candidate` reads as `RC1@candidate`, `chk:ref:ref:dog` as `ref:dog`, an evidence
/// item `chk:ev:c1:0` as `ev:c1:0`.
fn display_id(id: &str) -> &str {
    id.strip_prefix("chk:ob:")
        .or_else(|| id.strip_prefix("chk:ref:"))
        .or_else(|| id.strip_prefix("chk:"))
        .unwrap_or(id)
}

fn disposition_word(d: Disposition) -> &'static str {
    match d {
        Disposition::Violation => "violation",
        Disposition::Held => "held",
        Disposition::AllowedChange => "allowed_change",
        Disposition::Hidden => "hidden",
        Disposition::CantTell => "cant_tell",
        Disposition::Observed => "observed",
    }
}

fn confidence_word(c: Confidence) -> &'static str {
    match c {
        Confidence::Confident => "confident",
        Confidence::Tentative => "tentative",
    }
}

/// One line of text, whitespace collapsed, cut at `max` characters with an ellipsis.
fn one_line(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// What the item stands for: the rule's text for an obligation item, the reference image for a
/// reference item, the tool's region label for an evidence item.
fn item_label(run: &RunReport, id: &str) -> String {
    if let Some(instance) = id.strip_prefix("chk:ob:") {
        return run
            .instances
            .iter()
            .find(|i| i.id == instance)
            .map(|i| i.instance.text.clone())
            .unwrap_or_default();
    }
    if let Some(image) = id.strip_prefix("chk:ref:") {
        return format!("reference {image} as a whole");
    }
    let entry = run.checklist.get(id);
    match entry.and_then(|e| e.label.clone()) {
        Some(label) => format!("tool region: {label}"),
        None => "tool region".into(),
    }
}

fn item_line(run: &RunReport, item: &ItemRow, width: usize) -> String {
    let mut line = format!(
        "{:<width$}  {:<width_label$}  {} ({})",
        display_id(&item.id),
        one_line(&item_label(run, &item.id), LABEL_CHARS),
        disposition_word(item.disposition),
        confidence_word(item.confidence),
        width_label = LABEL_CHARS,
    );
    if let Some(by) = &item.allowed_by {
        let _ = write!(line, " by {by}");
    }
    if let Some(at) = item
        .root
        .as_ref()
        .map(root_location)
        .or_else(|| item.location.as_ref().map(location))
    {
        let _ = write!(line, " at {at}");
    }
    let reason = one_line(&item.reason, REASON_CHARS);
    if !reason.is_empty() {
        let _ = write!(line, ": {reason}");
    }
    line
}

fn finding_line(f: &FindingEntry) -> String {
    let disposition = serde_json::to_value(f.disposition)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let mut line = format!(
        "{} ({}) under {}",
        disposition,
        confidence_word(f.confidence),
        f.obligation_ids.join(", ")
    );
    if let Some(by) = &f.allowed_by {
        let _ = write!(line, " by {by}");
    }
    let at = f
        .root
        .as_ref()
        .map(root_location)
        .unwrap_or_else(|| location(&f.location));
    let _ = write!(line, " at {at}: {}", one_line(&f.reason, REASON_CHARS));
    line
}

fn fractions(values: &[f64]) -> String {
    values
        .iter()
        .map(|v| format!("{v:.2}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A location in the root image's frame: `candidate box [left top right bottom]` as fractions.
fn root_location(r: &RootLocation) -> String {
    match (r.kind, &r.bbox, &r.polygon) {
        (LocationKind::Bbox, Some(b), _) => format!("{} box [{}]", r.image, fractions(b)),
        (LocationKind::Polygon, _, Some(p)) => {
            format!("{} polygon of {} points", r.image, p.len())
        }
        (LocationKind::WholeImage, _, _) => format!("{} (whole image)", r.image),
        (LocationKind::Absent, _, _) => format!("{} (absent)", r.image),
        _ => match &r.bbox {
            Some(b) => format!("{} box [{}]", r.image, fractions(b)),
            None => r.image.clone(),
        },
    }
}

/// A location as submitted, in the frame the model named, when it did not map to a root.
fn location(l: &Location) -> String {
    match (l.kind, &l.bbox, &l.polygon) {
        (LocationKind::Bbox, Some(b), _) => format!("{} box [{}]", l.frame, fractions(b)),
        (LocationKind::Polygon, _, Some(p)) => {
            format!("{} polygon of {} points", l.frame, p.len())
        }
        (LocationKind::View, _, _) => format!(
            "{} view yaw {} pitch {} fov {}",
            l.frame,
            l.yaw.unwrap_or_default(),
            l.pitch.unwrap_or_default(),
            l.fov.unwrap_or_default()
        ),
        (LocationKind::WholeImage, _, _) => format!("{} (whole image)", l.frame),
        (LocationKind::Absent, _, _) => format!("{} (absent)", l.frame),
        _ => l.frame.clone(),
    }
}

/// Why an item's row is unsure, as the aggregation table reads it.
fn unsure_cause(item: &ItemRow) -> &'static str {
    match (item.disposition, item.confidence) {
        (Disposition::Violation, _) => "a tentative violation",
        (Disposition::CantTell, _) => "can't tell",
        (Disposition::Hidden, _) => "hidden",
        (Disposition::Held, _) => "held only tentatively",
        (Disposition::AllowedChange, _) => "an allowed change only tentatively",
        (Disposition::Observed, _) => "observed",
    }
}

/// The ids per cause, in first-seen order: `RC1@candidate, RC2@candidate held only tentatively`.
fn grouped(entries: Vec<(&'static str, String)>) -> String {
    let mut causes: Vec<(&'static str, Vec<String>)> = Vec::new();
    for (cause, id) in entries {
        match causes.iter_mut().find(|(c, _)| *c == cause) {
            Some((_, ids)) => {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
            None => causes.push((cause, vec![id])),
        }
    }
    causes
        .into_iter()
        .map(|(cause, ids)| format!("{} {cause}", ids.join(", ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// The finding's obligations, as the reason's subject.
fn finding_subject(f: &FindingEntry) -> String {
    format!("a finding under {}", f.obligation_ids.join(", "))
}

fn finding_unsure_cause(f: &FindingEntry) -> &'static str {
    use squint_core::submission::FindingDisposition as D;
    match f.disposition {
        D::Violation => "a tentative violation",
        D::CantTell => "can't tell",
        D::AllowedChange => "an allowed change only tentatively",
        D::Observed => "observed",
    }
}

/// Why the decision's verdict is what it is.
fn why(report: &Report) -> String {
    let rows_at = |level: Row| -> (Vec<&ItemRow>, Vec<&FindingEntry>) {
        let items = report
            .runs
            .iter()
            .flat_map(|r| r.items.iter())
            .filter(|i| i.row == level)
            .collect();
        // Observed findings have no aggregation row, so the rows pair with the others in order.
        let findings = report
            .runs
            .iter()
            .flat_map(|r| {
                r.findings
                    .iter()
                    .filter(|f| {
                        f.disposition != squint_core::submission::FindingDisposition::Observed
                    })
                    .zip(r.finding_rows.iter())
            })
            .filter(|(_, row)| **row == level)
            .map(|(f, _)| f)
            .collect();
        (items, findings)
    };
    match report.verdict {
        Some(Verdict::Fail) => {
            let (items, findings) = rows_at(Row::Fail);
            let mut named: Vec<String> = items
                .iter()
                .map(|i| display_id(&i.id).to_string())
                .collect();
            named.dedup();
            let mut parts = Vec::new();
            if !named.is_empty() {
                parts.push(format!("{} violated (confident)", named.join(", ")));
            }
            if !findings.is_empty() {
                let subjects: Vec<String> = findings.iter().map(|f| finding_subject(f)).collect();
                parts.push(format!("a confident violation in {}", subjects.join(", ")));
            }
            if parts.is_empty() {
                "fail".into()
            } else {
                format!("fail: {}", parts.join("; "))
            }
        }
        Some(Verdict::Unsure) => {
            let (items, findings) = rows_at(Row::Unsure);
            let mut entries: Vec<(&'static str, String)> = items
                .iter()
                .map(|i| (unsure_cause(i), display_id(&i.id).to_string()))
                .collect();
            entries.extend(
                findings
                    .iter()
                    .map(|f| (finding_unsure_cause(f), finding_subject(f))),
            );
            let mut parts = Vec::new();
            if !entries.is_empty() {
                parts.push(grouped(entries));
            }
            if !report.decision.disagreements.is_empty() {
                parts.push(format!(
                    "the two runs disagree ({})",
                    report.decision.disagreements.join("; ")
                ));
            }
            if parts.is_empty() {
                "unsure".into()
            } else {
                format!("unsure: {}", parts.join("; "))
            }
        }
        Some(Verdict::Pass) => {
            if report.runs.len() > 1 {
                "pass: every mandatory item held in both runs, and they agree".into()
            } else {
                "pass: every mandatory item held".into()
            }
        }
        None => {
            let errors: Vec<String> = report
                .runs
                .iter()
                .filter_map(|r| {
                    r.error
                        .as_ref()
                        .map(|e| format!("run {} ended in `{}`", r.run, e.class))
                })
                .collect();
            if errors.is_empty() {
                "no verdict: the decision did not complete".into()
            } else {
                format!("no verdict: {}", errors.join(", "))
            }
        }
    }
}

/// What the decision spent against its cap, and what the envelope reserved.
fn spend(report: &Report) -> String {
    let cost = &report.cost;
    let mut line = if report.cache.is_hit() {
        format!(
            "spent: $0 now (a cache hit; the decision it copies spent ${:.4})",
            cost.spent_usd
        )
    } else {
        format!(
            "spent: ${:.4} of the ${:.2} cap",
            cost.spent_usd, cost.cap_usd
        )
    };
    if cost.estimated {
        line.push_str(" (estimated: an answer reported no usage, so its bound counts)");
    }
    if let Some(envelope) = &cost.envelope
        && !report.cache.is_hit()
    {
        let _ = write!(
            line,
            "; the worst-case envelope reserved ${:.4} before the first request",
            envelope.usd
        );
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_line_collapses_and_cuts() {
        assert_eq!(one_line("a\n  b\tc", 10), "a b c");
        assert_eq!(one_line("abcdefghijkl", 5), "abcd…");
        assert_eq!(one_line("", 5), "");
    }

    #[test]
    fn display_ids_drop_the_checklist_prefix() {
        assert_eq!(display_id("chk:ob:RC1@candidate"), "RC1@candidate");
        assert_eq!(display_id("chk:ref:ref:dog"), "ref:dog");
        assert_eq!(display_id("chk:ev:c1:0"), "ev:c1:0");
    }

    #[test]
    fn causes_group_ids_in_first_seen_order() {
        assert_eq!(
            grouped(vec![
                ("held only tentatively", "RC1@candidate".into()),
                ("can't tell", "RC3@candidate".into()),
                ("held only tentatively", "RC2@candidate".into()),
            ]),
            "RC1@candidate, RC2@candidate held only tentatively; RC3@candidate can't tell"
        );
    }
}
