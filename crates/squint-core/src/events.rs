//! Typed journal events (SPEC "Report and journal"). The journal is the one record of a run; the
//! report is derived from these events, both at the end of a run and by `report rebuild` from a
//! partial journal. Every artifact an event references was written before the event.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::aggregate::{Row, Verdict};
use crate::artifacts::ArtifactRef;
use crate::checklist::Checklist;
use crate::image::CanonicalImage;
use crate::ledger::Envelope;
use crate::obligations::Instance;
use crate::provider::{StopReason, Usage};
use crate::report::ItemRow;
use crate::tool::Status;

/// One journal event, tagged by name with its data beside it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", rename_all = "snake_case")]
pub enum Event {
    Resolved(Resolved),
    Prepared(Prepared),
    Reserved(Reserved),
    TurnSent(TurnSent),
    Retry(Retry),
    AttemptFailed(AttemptFailed),
    TurnReceived(TurnReceived),
    TurnExcluded(TurnExcluded),
    ToolCalled(ToolCalled),
    ToolReturned(ToolReturned),
    EvidenceDropped(EvidenceDropped),
    Repair(Repair),
    Submission(Submission),
    Verdict(VerdictReached),
    Error(Terminal),
    Cancelled(Terminal),
}

impl Event {
    /// The event name as written in the journal.
    pub fn name(&self) -> &'static str {
        match self {
            Event::Resolved(_) => "resolved",
            Event::Prepared(_) => "prepared",
            Event::Reserved(_) => "reserved",
            Event::TurnSent(_) => "turn_sent",
            Event::Retry(_) => "retry",
            Event::AttemptFailed(_) => "attempt_failed",
            Event::TurnReceived(_) => "turn_received",
            Event::TurnExcluded(_) => "turn_excluded",
            Event::ToolCalled(_) => "tool_called",
            Event::ToolReturned(_) => "tool_returned",
            Event::EvidenceDropped(_) => "evidence_dropped",
            Event::Repair(_) => "repair",
            Event::Submission(_) => "submission",
            Event::Verdict(_) => "verdict",
            Event::Error(_) => "error",
            Event::Cancelled(_) => "cancelled",
        }
    }
}

/// Step 1 done: images decoded, instances resolved, the static checklist built, tools frozen.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resolved {
    pub images: Vec<CanonicalImage>,
    /// The job images as sent, by frame id.
    pub images_sent: BTreeMap<String, ArtifactRef>,
    pub instances: Vec<Instance>,
    pub checklist: Checklist,
    pub tools: Vec<String>,
}

/// Step 2: the preparation the images went through (`prep_version` names the provider's).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prepared {
    pub prep_version: String,
    pub images: usize,
}

/// Step 3: the decision envelope reserved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reserved {
    pub envelope: Envelope,
    pub cap_usd: f64,
    pub shared_cap_usd: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnSent {
    pub turn: u32,
    pub tools: Vec<String>,
    pub history_messages: usize,
    pub max_output_tokens: u32,
    pub bound_usd: f64,
    pub request: ArtifactRef,
}

/// A transport failure about to be retried; the failed attempt is charged at its bound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Retry {
    pub turn: u32,
    pub attempt: u32,
    pub status: Option<u16>,
    pub delay_ms: u64,
    pub error: String,
    pub charged_usd: f64,
}

/// The last attempt of a turn failed (retries exhausted, a non-retryable status, the server
/// ended the conversation): its usage is unknown and it is charged at the bound.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttemptFailed {
    pub turn: u32,
    /// Attempts made in all for this turn, this one included.
    pub attempts: u32,
    pub class: String,
    pub error: String,
    pub charged_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnReceived {
    pub turn: u32,
    pub stop_reason: StopReason,
    pub tool_calls: Vec<String>,
    pub usage: Usage,
    /// The answer reported no usage, so its cost is the turn's bound, not a measured figure.
    #[serde(default)]
    pub estimated: bool,
    pub provider_request_id: Option<String>,
    pub attempts: u32,
    pub cost_usd: f64,
    pub response: ArtifactRef,
}

/// A non-replayable response: it never enters provider history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnExcluded {
    pub turn: u32,
    pub stop_reason: StopReason,
    pub reason: String,
    pub text_blocks: usize,
    pub tool_calls: usize,
}

/// A call the model made. Executed calls carry a `call_id`; a call refused for the call budget or
/// on the final turn carries the `reason` instead; a call with bad arguments carries
/// `invalid_args`. A call the tool itself refused has two lines under one `call_id`: the dispatch,
/// then the refusal with `executed: true`, the envelope's status and the duration. The report
/// folds them into one entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCalled {
    pub call_id: Option<String>,
    pub provider_call_id: String,
    pub tool: String,
    /// The manifest version, once the tool was looked up.
    pub version: Option<String>,
    pub arguments: Option<serde_json::Value>,
    pub executed: bool,
    pub reason: Option<String>,
    pub invalid_args: Option<String>,
    pub status: Option<Status>,
    pub duration_ms: Option<u64>,
    /// The evaluation projection the tool ran under, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReturnedImage {
    pub frame: String,
    pub artifact: Option<ArtifactRef>,
    /// The frame the image was cut from (or, for an aligned frame, warped into), how it sits in
    /// it, its root image and its size: what the registry recorded, so a report can rebuild the
    /// frame tree.
    pub parent: String,
    pub root: String,
    pub mapping: crate::frames::Mapping,
    pub width: u32,
    pub height: u32,
}

/// An executed call's result, ingested.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolReturned {
    pub call_id: String,
    pub status: Status,
    pub regions: usize,
    pub images: Vec<ReturnedImage>,
    /// Checklist items the result added, and the whole checklist after ingestion.
    pub added: Vec<String>,
    pub checklist: Checklist,
    pub duration_ms: u64,
    /// The structured result in full.
    pub envelope: ArtifactRef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceDropped {
    pub call_id: String,
    pub region_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repair {
    pub form: String,
    pub reason: String,
    pub after_turn: u32,
    pub repairs_used: u32,
}

/// A submission that validated, with its aggregation rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Submission {
    pub provider_call_id: String,
    pub items: Vec<ItemRow>,
    pub findings: usize,
    pub finding_rows: Vec<Row>,
    /// Every finding with its location as submitted and in root coordinates.
    #[serde(default)]
    pub finding_entries: Vec<crate::report::FindingEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerdictReached {
    pub verdict: Verdict,
}

/// How a run ended short of a verdict: `error` or `cancelled`, with the state reached.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Terminal {
    pub class: String,
    pub message: String,
    pub state: Option<String>,
}
