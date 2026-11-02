//! The report (SPEC "Report and journal"): one JSON document per decision, derived from the
//! decision header (`decision.json`, written when the decision starts) and the runs' journals. The
//! same derivation serves the end of a run and `squint report rebuild` on a partial journal, so
//! the journal is the one record. `schemas/report-v1.json` is generated from these types.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::aggregate::{Confidence, Disposition, Row, Verdict};
use crate::artifacts::ArtifactRef;
use crate::cache::CacheStatus;
use crate::checklist::Checklist;
use crate::config::PolicyConfig;
use crate::decision::{self, Decision, DecisionItem, RunSummary};
use crate::events::Event;
use crate::frames::Location;
use crate::image::CanonicalImage;
use crate::job::JobSummary;
use crate::journal::{Journal, Line};
use crate::ledger::Envelope;
use crate::obligations::Instance;
use crate::provider::{StopReason, Usage};
use crate::run::RulesSummary;
use crate::{Error, Result};

pub const REPORT_VERSION: u32 = 1;
/// The decision header's file name under the decision directory.
pub const HEADER_FILE: &str = "decision.json";
/// The report's file name under the decision directory.
pub const REPORT_FILE: &str = "report.json";

/// Terminal states of a run as the report names them.
pub const TERMINAL_VERDICT: &str = "verdict";
pub const TERMINAL_ERROR: &str = "error";
pub const TERMINAL_CANCELLED: &str = "cancelled";
/// The journal ends before any terminal event (the process died or is still running).
pub const TERMINAL_INCOMPLETE: &str = "incomplete";

/// What the report needs that no run journal carries; written before the first run.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DecisionHeader {
    pub report_version: u32,
    pub harness_version: String,
    pub use_case: String,
    /// The job as resolved (base64 payloads summarised).
    pub job: JobSummary,
    /// The config as resolved, with every expanded secret redacted.
    pub config: serde_json::Value,
    /// The rules as merged; `None` when they did not resolve.
    pub rules: Option<RulesSummary>,
    pub prompt_version: String,
    pub provider: String,
    /// Enabled tools as `name@version`, in prompt order.
    pub tools: Vec<String>,
    pub decision_runs: u32,
    pub policy: PolicyConfig,
    pub cap_usd: f64,
    /// The planned decision envelope; `None` when it could not be planned.
    pub envelope: Option<Envelope>,
    /// How the cache figured: on a hit, `source` names the decision this one is a copy of.
    pub cache: CacheStatus,
    /// Who receives the data (SPEC "Data leaving the machine"); `None` for the scripted provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient: Option<Recipient>,
}

/// The recipient chain of a decision's requests: the profile, the host the requests go to, the
/// upstream provider when a gateway reports one, and the resolved model id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recipient {
    pub profile: String,
    pub host: String,
    pub upstream: Option<String>,
    pub model: String,
    /// The API version or path the adapter speaks (`2023-06-01`, `v1beta`, `v1`).
    pub api_version: String,
}

impl DecisionHeader {
    pub fn write(&self, dir: &Path) -> Result<()> {
        crate::artifacts::write_private(
            &dir.join(HEADER_FILE),
            serde_json::to_string_pretty(self)?.as_bytes(),
        )
    }

    pub fn read(dir: &Path) -> Result<DecisionHeader> {
        let path = dir.join(HEADER_FILE);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            Error::Io(std::io::Error::new(
                e.kind(),
                format!("{}: {e}", path.display()),
            ))
        })?;
        Ok(serde_json::from_str(&text)?)
    }
}

/// The exit code under the policy, except that an `invalid` request (the contract's status for a
/// job that does not resolve or an image that is not the one named) exits 64 whatever the policy.
fn exit_code(verdict: Option<Verdict>, policy: &PolicyConfig, runs: &[RunReport]) -> i32 {
    let invalid = verdict.is_none()
        && runs.iter().find_map(|r| r.error.as_ref()).is_some_and(|e| {
            crate::contract::status_of(&e.class).0 == table_manners::Status::Invalid
        });
    if invalid {
        crate::contract::EXIT_INVALID
    } else {
        decision::exit_code(verdict, policy)
    }
}

/// The decision-level report.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub report_version: u32,
    pub harness_version: String,
    pub use_case: String,
    pub job: JobSummary,
    pub config: serde_json::Value,
    pub rules: Option<RulesSummary>,
    pub prompt_version: String,
    pub provider: String,
    pub tools: Vec<String>,
    /// How many runs the decision asked for, and the runs found, in order.
    pub decision_runs: u32,
    pub runs: Vec<RunReport>,
    pub decision: Decision,
    /// The combined verdict, `None` when the decision is an error.
    pub verdict: Option<Verdict>,
    pub policy: PolicyConfig,
    pub exit_code: i32,
    /// The decision envelope and what it came to.
    pub cost: CostSummary,
    pub cache: CacheStatus,
}

/// Money for the decision (SPEC "Cost ledger"), derived from the journals.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct CostSummary {
    /// The planned envelope, `None` when it could not be planned.
    pub envelope: Option<Envelope>,
    pub cap_usd: f64,
    /// Settled usage plus every failed attempt charged at its bound.
    pub spent_usd: f64,
    /// The part of `spent_usd` no provider answer settled: attempts sent with no usage reported,
    /// or cancelled in flight, counted at their bound (table-manners 2.3).
    #[serde(default)]
    pub unresolved_usd: f64,
    /// Some answer reported no usage and counts its bound: the spend is estimated, not measured.
    #[serde(default)]
    pub estimated: bool,
    /// What the envelope gave back.
    pub released_usd: f64,
    /// Spend beyond the envelope (bounds are estimates).
    pub overrun_usd: f64,
    /// Billable attempts made.
    pub attempts: u32,
}

/// A submitted finding in the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindingEntry {
    pub obligation_ids: Vec<String>,
    pub disposition: crate::submission::FindingDisposition,
    pub confidence: crate::aggregate::Confidence,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_by: Option<String>,
    /// As submitted, in the frame the model named.
    pub location: crate::frames::Location,
    /// Mapped to the root image's raw frame; `None` when the location did not resolve.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<RootLocation>,
}

/// A location in root coordinates: the raw frame of the root image.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RootLocation {
    /// The root image's id (`candidate`, `source`, `ref:<label>`).
    pub image: String,
    pub frame: String,
    pub kind: crate::frames::LocationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[f64; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub polygon: Option<Vec<[f64; 2]>>,
    /// The mapping fell partly outside a parent and was clipped.
    pub partial: bool,
    /// A view footprint that crossed the panorama's seam: its polygons on both sides, for the
    /// contract's location (`polygon` holds the one side the registry keeps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seam_parts: Option<Vec<Vec<[f64; 2]>>>,
}

/// A frame of the run's registry: a raw frame of a job image, or a crop a tool returned, with
/// how it sits in its parent, so locations can be mapped to the root image's raw coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameEntry {
    /// The root image's id.
    pub root: String,
    /// The parent frame of a derived frame; `None` for a raw frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mapping: Option<crate::frames::Mapping>,
    pub width: u32,
    pub height: u32,
    pub projection: crate::job::Projection,
}

/// One run's report.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunReport {
    pub run: u32,
    /// `verdict`, `error`, `cancelled` or `incomplete`.
    pub terminal: String,
    pub images: Vec<CanonicalImage>,
    /// Every image the model saw, by frame id: the job images as sent and every image a tool
    /// returned.
    pub images_sent: BTreeMap<String, ArtifactRef>,
    /// The preparation the images went through (the `prepared` event): the provider's
    /// `prep_version`, `+many<edge>` when resolve prepared them at the many-image size. `None`
    /// when the run ended before its images were prepared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prep_version: Option<String>,
    /// The frame registry: raw frames and every crop a tool returned, by frame id.
    #[serde(default)]
    pub frames: BTreeMap<String, FrameEntry>,
    pub instances: Vec<InstanceEntry>,
    /// The checklist as it stood at the end (static items plus every evidence item added).
    pub checklist: Checklist,
    pub turns: Vec<TurnEntry>,
    pub tool_calls: Vec<ToolCallEntry>,
    /// Repairs taken, in order (at most `budget.repairs`).
    pub repairs: Vec<RepairEntry>,
    pub items: Vec<ItemRow>,
    /// Aggregation rows of the submission's findings (observed findings produce none).
    pub finding_rows: Vec<Row>,
    /// Every finding of the submission, with its location as submitted and mapped to the root
    /// image's raw frame, for localisation scoring and the viewer.
    #[serde(default)]
    pub findings: Vec<FindingEntry>,
    pub verdict: Option<Verdict>,
    pub error: Option<ErrorEntry>,
    /// Wall time from the first journal line to the last, in milliseconds.
    pub wall_ms: u64,
}

/// One model turn: a request, and its response if one came back.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnEntry {
    pub turn: u32,
    /// The output allowance of the request (raised to `repair_output_tokens` on a reissue).
    pub max_output_tokens: u32,
    /// The bound of one attempt at catalog prices.
    pub bound_usd: f64,
    /// The model-visible request body, redacted and bounded.
    pub request: ArtifactRef,
    /// `None` while no response came back.
    pub stop_reason: Option<StopReason>,
    pub tool_calls: Vec<String>,
    pub usage: Usage,
    pub provider_request_id: Option<String>,
    /// Billable attempts (1 plus transport retries).
    pub attempts: u32,
    /// Settled usage plus every failed attempt charged at its bound.
    pub cost_usd: f64,
    /// The part of `cost_usd` charged at the bound because no answer settled it.
    #[serde(default)]
    pub unresolved_usd: f64,
    /// The answer reported no usage, so its cost is the turn's bound, estimated.
    #[serde(default)]
    pub estimated: bool,
    /// Whether the response entered provider history (false for an excluded or unanswered turn).
    pub entered_history: bool,
    /// The model-visible response body, redacted and bounded.
    pub response: Option<ArtifactRef>,
}

/// One repair (SPEC "Repair protocol").
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RepairEntry {
    /// `rejected` (the response stayed in history, error results answered it) or `reissue` (the
    /// response was excluded and the request sent again).
    pub form: String,
    /// `invalid_submission`, `mixed_batch`, `stray_call`, `truncated`, `refusal` or `no_call`.
    pub reason: String,
    /// The turn number whose response was repaired.
    pub after_turn: u32,
}

/// One tool call the model made: executed, rejected for its arguments, or refused (call budget,
/// final turn). A refused call has no harness id; its `id` is the provider's.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToolCallEntry {
    pub id: String,
    pub provider_call_id: String,
    pub tool: String,
    pub version: Option<String>,
    pub executed: bool,
    /// Why a call was refused without execution (`call budget`, `final_turn`).
    pub reason: Option<String>,
    pub arguments: serde_json::Value,
    pub status: Option<crate::tool::Status>,
    pub invalid_args: Option<String>,
    pub duration_ms: u64,
    pub added_items: Vec<String>,
    pub dropped: Vec<String>,
    /// The structured result in full; `None` when the call was not executed.
    pub envelope: Option<ArtifactRef>,
    /// The evaluation projection the tool ran under (SPEC "Evaluation projections"), if any: the
    /// envelope stored is the projected one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstanceEntry {
    pub id: String,
    pub instance: Instance,
}

/// A dispositioned checklist item with its aggregation row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ItemRow {
    pub id: String,
    pub disposition: Disposition,
    pub confidence: Confidence,
    pub reason: String,
    pub allowed_by: Option<String>,
    /// As submitted, in the frame the model named.
    pub location: Option<Location>,
    /// The location mapped to the root image's raw frame; `None` without a location or when it
    /// did not resolve. Violations carry one, and the localisation scorer reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<RootLocation>,
    pub row: Row,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ErrorEntry {
    pub class: String,
    pub message: String,
}

impl RunReport {
    /// Derives a run's report from its journal, complete or partial.
    pub fn from_lines(run: u32, lines: &[Line]) -> RunReport {
        let mut r = RunReport {
            run,
            terminal: TERMINAL_INCOMPLETE.into(),
            images: Vec::new(),
            images_sent: BTreeMap::new(),
            prep_version: None,
            frames: BTreeMap::new(),
            instances: Vec::new(),
            checklist: Checklist::default(),
            turns: Vec::new(),
            tool_calls: Vec::new(),
            repairs: Vec::new(),
            items: Vec::new(),
            finding_rows: Vec::new(),
            findings: Vec::new(),
            verdict: None,
            error: None,
            wall_ms: match (lines.first(), lines.last()) {
                (Some(a), Some(b)) => b.unix_ms.saturating_sub(a.unix_ms),
                _ => 0,
            },
        };
        for line in lines {
            match &line.event {
                Event::Resolved(e) => {
                    r.images = e.images.clone();
                    r.images_sent = e.images_sent.clone();
                    for img in &e.images {
                        r.frames.insert(
                            format!("raw:{}", img.id),
                            FrameEntry {
                                root: img.id.clone(),
                                parent: None,
                                mapping: None,
                                width: img.width,
                                height: img.height,
                                projection: img.projection,
                            },
                        );
                    }
                    r.instances = e
                        .instances
                        .iter()
                        .map(|i| InstanceEntry {
                            id: i.id(),
                            instance: i.clone(),
                        })
                        .collect();
                    r.checklist = e.checklist.clone();
                }
                Event::Prepared(e) => r.prep_version = Some(e.prep_version.clone()),
                Event::Reserved(_) => {}
                Event::TurnSent(e) => r.turns.push(TurnEntry {
                    turn: e.turn,
                    max_output_tokens: e.max_output_tokens,
                    bound_usd: e.bound_usd,
                    request: e.request.clone(),
                    stop_reason: None,
                    tool_calls: Vec::new(),
                    usage: Usage::default(),
                    provider_request_id: None,
                    attempts: 1,
                    cost_usd: 0.0,
                    unresolved_usd: 0.0,
                    estimated: false,
                    entered_history: false,
                    response: None,
                }),
                Event::Retry(e) => {
                    if let Some(t) = r.turns.iter_mut().find(|t| t.turn == e.turn) {
                        t.attempts += 1;
                        t.cost_usd += e.charged_usd;
                        t.unresolved_usd += e.charged_usd;
                    }
                }
                Event::AttemptFailed(e) => {
                    if let Some(t) = r.turns.iter_mut().find(|t| t.turn == e.turn) {
                        t.attempts = e.attempts;
                        t.cost_usd += e.charged_usd;
                        t.unresolved_usd += e.charged_usd;
                    }
                }
                Event::TurnReceived(e) => {
                    if let Some(t) = r.turns.iter_mut().find(|t| t.turn == e.turn) {
                        t.stop_reason = Some(e.stop_reason.clone());
                        t.tool_calls = e.tool_calls.clone();
                        t.usage = e.usage;
                        t.provider_request_id = e.provider_request_id.clone();
                        t.attempts = e.attempts;
                        t.cost_usd = e.cost_usd;
                        t.estimated = e.estimated;
                        t.entered_history = true;
                        t.response = Some(e.response.clone());
                    }
                }
                Event::TurnExcluded(e) => {
                    if let Some(t) = r.turns.iter_mut().find(|t| t.turn == e.turn) {
                        t.entered_history = false;
                    }
                }
                Event::ToolCalled(e) => {
                    let id = e
                        .call_id
                        .clone()
                        .unwrap_or_else(|| e.provider_call_id.clone());
                    match r
                        .tool_calls
                        .iter_mut()
                        .find(|c| c.id == id && e.call_id.is_some())
                    {
                        Some(c) => {
                            if e.invalid_args.is_some() {
                                c.invalid_args = e.invalid_args.clone();
                            }
                            if e.status.is_some() {
                                c.status = e.status;
                            }
                            if let Some(d) = e.duration_ms {
                                c.duration_ms = d;
                            }
                            if e.version.is_some() {
                                c.version = e.version.clone();
                            }
                            if e.projection.is_some() {
                                c.projection = e.projection.clone();
                            }
                            c.executed = e.executed;
                        }
                        None => r.tool_calls.push(ToolCallEntry {
                            id,
                            provider_call_id: e.provider_call_id.clone(),
                            tool: e.tool.clone(),
                            version: e.version.clone(),
                            executed: e.executed,
                            reason: e.reason.clone(),
                            arguments: e.arguments.clone().unwrap_or(serde_json::Value::Null),
                            status: e.status,
                            invalid_args: e.invalid_args.clone(),
                            duration_ms: e.duration_ms.unwrap_or(0),
                            added_items: Vec::new(),
                            dropped: Vec::new(),
                            envelope: None,
                            projection: e.projection.clone(),
                        }),
                    }
                }
                Event::ToolReturned(e) => {
                    if let Some(c) = r.tool_calls.iter_mut().find(|c| c.id == e.call_id) {
                        c.status = Some(e.status);
                        c.duration_ms = e.duration_ms;
                        c.added_items = e.added.clone();
                        c.envelope = Some(e.envelope.clone());
                    }
                    r.checklist = e.checklist.clone();
                    for img in &e.images {
                        if let Some(a) = &img.artifact {
                            r.images_sent.insert(img.frame.clone(), a.clone());
                        }
                        // As the registry does: a box keeps the parent's projection, a view is
                        // flat; the root is the image the pixels came from.
                        let parent = r.frames.get(&img.parent);
                        let root = img.root.clone();
                        let projection = match &img.mapping {
                            crate::frames::Mapping::Bbox(_)
                            | crate::frames::Mapping::Homography(_)
                            | crate::frames::Mapping::Rotation(_) => parent
                                .map(|p| p.projection)
                                .unwrap_or(crate::job::Projection::Flat),
                            crate::frames::Mapping::View { .. } => crate::job::Projection::Flat,
                        };
                        r.frames.insert(
                            img.frame.clone(),
                            FrameEntry {
                                root,
                                parent: Some(img.parent.clone()),
                                mapping: Some(img.mapping.clone()),
                                width: img.width,
                                height: img.height,
                                projection,
                            },
                        );
                    }
                }
                Event::EvidenceDropped(e) => {
                    if let Some(c) = r.tool_calls.iter_mut().find(|c| c.id == e.call_id) {
                        c.dropped.push(e.region_id.clone());
                    }
                }
                Event::Repair(e) => r.repairs.push(RepairEntry {
                    form: e.form.clone(),
                    reason: e.reason.clone(),
                    after_turn: e.after_turn,
                }),
                Event::Submission(e) => {
                    r.items = e.items.clone();
                    r.finding_rows = e.finding_rows.clone();
                    r.findings = e.finding_entries.clone();
                }
                Event::Verdict(e) => {
                    r.verdict = Some(e.verdict);
                    r.terminal = TERMINAL_VERDICT.into();
                }
                Event::Error(e) => {
                    r.error = Some(ErrorEntry {
                        class: e.class.clone(),
                        message: e.message.clone(),
                    });
                    r.terminal = TERMINAL_ERROR.into();
                }
                Event::Cancelled(e) => {
                    r.error = Some(ErrorEntry {
                        class: e.class.clone(),
                        message: e.message.clone(),
                    });
                    r.terminal = TERMINAL_CANCELLED.into();
                }
            }
        }
        if r.terminal == TERMINAL_INCOMPLETE {
            r.error = Some(ErrorEntry {
                class: TERMINAL_INCOMPLETE.into(),
                message: match lines.last() {
                    Some(l) => format!("the journal ends after `{}`", l.event.name()),
                    None => "the journal is empty".into(),
                },
            });
        }
        r
    }

    /// What the decision table needs from this run.
    pub fn summary(&self) -> RunSummary {
        RunSummary {
            verdict: self.verdict,
            items: self
                .items
                .iter()
                .map(|row| {
                    let entry = self.checklist.get(&row.id);
                    DecisionItem {
                        id: row.id.clone(),
                        disposition: row.disposition,
                        confidence: row.confidence,
                        allowed_by: row.allowed_by.clone(),
                        root_image: entry.and_then(|c| c.root_image.clone()),
                        instance_ids: entry.map(|c| c.instance_ids.clone()).unwrap_or_default(),
                        root_location: entry.and_then(|c| c.root_location.clone()),
                    }
                })
                .collect(),
        }
    }
}

impl Report {
    /// Derives the report from a decision directory: the header plus every run journal present,
    /// complete or partial. Missing runs make the decision an error.
    pub fn rebuild(dir: &Path) -> Result<Report> {
        let header = DecisionHeader::read(dir)?;
        if header.cache.is_hit() {
            // A hit holds no runs of its own: the decision is the source's, with this cache status.
            let source = header.cache.source.clone().ok_or_else(|| {
                Error::Io(std::io::Error::other(format!(
                    "{}: a cache hit without a source",
                    dir.join(HEADER_FILE).display()
                )))
            })?;
            // The policy, config and cache settings are outside the key: this decision's apply.
            let mut report = Report::rebuild(&source)?;
            report.cache = header.cache;
            report.policy = header.policy;
            report.config = header.config;
            report.exit_code = exit_code(report.verdict, &report.policy, &report.runs);
            return Ok(report);
        }
        let mut runs = Vec::new();
        let mut envelope: Option<Envelope> = header.envelope.clone();
        for n in 1..=header.decision_runs {
            let path = dir.join(format!("run-{n}")).join("journal.jsonl");
            if !path.exists() {
                break;
            }
            let lines = Journal::read(&path)?;
            if envelope.is_none() {
                envelope = lines.iter().find_map(|l| match &l.event {
                    Event::Reserved(r) => Some(r.envelope.clone()),
                    _ => None,
                });
            }
            runs.push(RunReport::from_lines(n, &lines));
        }
        let summaries: Vec<RunSummary> = runs.iter().map(RunReport::summary).collect();
        let decision = if (runs.len() as u32) < header.decision_runs {
            Decision {
                verdict: None,
                error: true,
                agree: None,
                disagreements: Vec::new(),
            }
        } else {
            match summaries.as_slice() {
                [one] => decision::single(one),
                [a, b] => decision::combine(a, b),
                _ => Decision {
                    verdict: None,
                    error: true,
                    agree: None,
                    disagreements: Vec::new(),
                },
            }
        };
        let exit_code = exit_code(decision.verdict, &header.policy, &runs);
        let spent_usd: f64 = runs
            .iter()
            .flat_map(|r| r.turns.iter())
            .map(|t| t.cost_usd)
            .sum();
        let unresolved_usd: f64 = runs
            .iter()
            .flat_map(|r| r.turns.iter())
            .map(|t| t.unresolved_usd)
            // From +0.0: an empty f64 sum is -0.0.
            .fold(0.0, |a, b| a + b);
        let attempts: u32 = runs
            .iter()
            .flat_map(|r| r.turns.iter())
            .map(|t| t.attempts)
            .sum();
        let envelope_usd = envelope.as_ref().map(|e| e.usd).unwrap_or(0.0);
        let cost = CostSummary {
            envelope,
            cap_usd: header.cap_usd,
            spent_usd,
            unresolved_usd,
            estimated: runs
                .iter()
                .flat_map(|r| r.turns.iter())
                .any(|t| t.estimated),
            released_usd: (envelope_usd - spent_usd).max(0.0),
            overrun_usd: (spent_usd - envelope_usd).max(0.0),
            attempts,
        };
        Ok(Report {
            report_version: header.report_version,
            harness_version: header.harness_version,
            use_case: header.use_case,
            job: header.job,
            config: header.config,
            rules: header.rules,
            prompt_version: header.prompt_version,
            provider: header.provider,
            tools: header.tools,
            decision_runs: header.decision_runs,
            runs,
            verdict: decision.verdict,
            decision,
            policy: header.policy,
            exit_code,
            cost,
            cache: header.cache,
        })
    }

    /// Writes `report.json` into the decision directory.
    pub fn write(&self, dir: &Path) -> Result<()> {
        crate::artifacts::write_private(
            &dir.join(REPORT_FILE),
            serde_json::to_string_pretty(self)?.as_bytes(),
        )
    }

    /// Reads a written `report.json`.
    pub fn read(dir: &Path) -> Result<Report> {
        Ok(serde_json::from_str(&std::fs::read_to_string(
            dir.join(REPORT_FILE),
        )?)?)
    }

    /// The JSON Schema of the report (`schemas/report-v1.json`).
    pub fn json_schema() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(Report)).unwrap_or_default()
    }
}

/// Removes decision directories under `out_dir` whose age is at least `older_than`. Age comes
/// from the directory name (`decision-<unix ms>-<pid>`), falling back to the modification time.
/// Returns what was removed.
pub fn prune(out_dir: &Path, older_than: Duration, now: SystemTime) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    for entry in std::fs::read_dir(out_dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !path.is_dir() || !name.starts_with("decision-") {
            continue;
        }
        let started = name
            .split('-')
            .nth(1)
            .and_then(|ms| ms.parse::<u64>().ok())
            .map(|ms| UNIX_EPOCH + Duration::from_millis(ms))
            .or_else(|| std::fs::metadata(&path).and_then(|m| m.modified()).ok());
        let Some(started) = started else { continue };
        let age = now.duration_since(started).unwrap_or(Duration::ZERO);
        if age >= older_than {
            std::fs::remove_dir_all(&path)?;
            removed.push(path);
        }
    }
    removed.sort();
    Ok(removed)
}
