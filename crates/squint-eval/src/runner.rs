//! The matrix runner (SPEC "Runner and arms"): every item of every set through every cell,
//! `repetitions` times, never reading the cache, under the durable evaluation ledger, with the
//! execution descriptor written before the first decision and checked on resume. Decisions
//! live in immutable directories under `results/`; a complete one holds `result.json` and is
//! skipped on resume, a started one without it is marked partial and run again in a fresh
//! attempt directory (its reservation was charged at bound by the ledger's recovery).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use squint_core::config::Config;
use squint_core::ledger::Ledger;
use squint_core::provider::Provider;
use squint_core::run::{self, Cancel, Services};
use squint_core::tool::ToolRegistry;
use squint_providers::catalog::Catalog;

use crate::admissions::Admissions;
use crate::arms::{ToolSelection, tools_for};
use crate::descriptor::{Descriptor, Inputs, WriterLock};
use crate::eval_ledger::EvalLedger;
use crate::matrix::{Cell, Matrix};
use crate::sets::Set;
use crate::{Error, Result};

pub const SUMMARY_FILE: &str = "summary.json";
pub const RESULT_FILE: &str = "result.json";

/// Everything a run needs besides the providers and tools.
pub struct Plan {
    pub eval_dir: PathBuf,
    pub matrix: Matrix,
    pub matrix_text: String,
    pub config_text: String,
    pub evalsets_root: PathBuf,
    /// Needed for arm D; `None` means no tool is admitted.
    pub admissions: Option<Admissions>,
    pub harness_version: String,
    /// Admit the fake profile (tests and the testkit build).
    pub allow_fake: bool,
}

/// How providers and tool registries are made per cell: real ones in the binary, scripted ones
/// in tests.
pub trait Factories {
    fn provider(&self, cell: &Cell, config: &Config) -> Result<Box<dyn Provider>>;
    fn tools(&self, selection: &ToolSelection, config: &Config) -> Result<ToolRegistry>;
    /// The lock digest of the pack each enabled tool comes from, by tool name; built-in and
    /// scripted tools have none.
    fn lock_digests(&self, _config: &Config) -> BTreeMap<String, String> {
        BTreeMap::new()
    }
}

/// One decision's outcome, as `result.json` in its directory and as a row of the summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionResult {
    pub id: String,
    pub set: String,
    pub item: String,
    pub cell: String,
    pub repetition: u32,
    /// Repetition 1: the one every gate and headline number uses.
    pub decision_repetition: bool,
    pub verdict: Option<String>,
    pub exit_code: i32,
    pub error_class: Option<String>,
    pub error: Option<String>,
    pub cost_usd: f64,
    pub wall_ms: u64,
    /// The decision directory, relative to the evaluation directory.
    pub dir: String,
}

/// `summary.json`: every decision of the matrix as it stands, plus what this run did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub summary_version: u32,
    pub matrix: String,
    pub matrix_identity: String,
    pub descriptor_sha256: String,
    pub decisions: Vec<DecisionResult>,
    /// Decisions this run completed.
    pub completed: usize,
    /// Decisions already complete when this run started.
    pub skipped: usize,
    /// Decisions found started but incomplete at the start of this run, and run again.
    pub partial: Vec<String>,
    pub cancelled: bool,
    /// The cap no longer covered the next decision's envelope: the run stopped there, without a
    /// row for it or anything after it. A resume tries again; a larger cap is a new matrix.
    #[serde(default)]
    pub budget_exhausted: bool,
    /// The ledger's cumulative spend across every run of this evaluation.
    pub spent_usd: f64,
    pub cap_usd: f64,
    /// The paired bootstrap's seed and resamples when the matrix froze them (`[matrix.gate]`),
    /// so the report's paired intervals reproduce.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bootstrap: Option<crate::metrics::BootstrapSpec>,
}

fn io(path: &Path, e: std::io::Error) -> Error {
    Error::Runner(format!("{}: {e}", path.display()))
}

fn safe(segment: &str) -> String {
    segment.replace(['/', ':'], "_")
}

/// The decision directory as the summary records it: relative to the evaluation directory, so a
/// published summary carries no local path. The run makes the directory absolute (a pack is
/// restarted in it), while the evaluation directory is whatever the command line said.
pub fn relative_dir(decision: &Path, eval_dir: &Path) -> String {
    let absolute = std::path::absolute(eval_dir).unwrap_or_else(|_| eval_dir.to_path_buf());
    decision
        .strip_prefix(&absolute)
        .or_else(|_| decision.strip_prefix(eval_dir))
        .unwrap_or(decision)
        .display()
        .to_string()
}

fn decision_dir(cell: &Cell, set_id: &str, item: &str, repetition: u32) -> PathBuf {
    PathBuf::from("results")
        .join(set_id)
        .join(item)
        .join(safe(&format!(
            "{}_{}",
            cell.model.profile, cell.model.model
        )))
        .join(cell.arm.to_string())
        .join(format!("rep-{repetition}"))
}

fn toml_scalar(v: &toml::Value) -> String {
    match v {
        toml::Value::String(s) => format!("{s:?}"),
        other => other.to_string(),
    }
}

/// The per-cell config: the base config with the provider, tools and rules set for the cell
/// and the set, and the matrix's decision runs.
pub(crate) fn cell_config(
    plan: &Plan,
    cell: &Cell,
    set: &Set,
    selection: &ToolSelection,
) -> Result<Config> {
    let mut overrides = vec![
        format!("provider.profile={:?}", cell.model.profile),
        format!("provider.model={:?}", cell.model.model),
        format!("rules.base={:?}", set.manifest.set.rules.base),
        format!("budget.decision_runs={}", plan.matrix.matrix.decision_runs),
        format!(
            "tools.enabled=[{}]",
            selection
                .enabled
                .iter()
                .map(|t| format!("{t:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        format!(
            "tools.order=[{}]",
            selection
                .enabled
                .iter()
                .map(|t| format!("{t:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    ];
    if let Some(account) = &cell.model.account {
        overrides.push(format!("privacy.account={account:?}"));
    }
    if !cell.model.controls.is_empty() {
        let inline: Vec<String> = cell
            .model
            .controls
            .iter()
            .map(|(k, v)| format!("{k:?} = {}", toml_scalar(v)))
            .collect();
        overrides.push(format!("provider.controls={{ {} }}", inline.join(", ")));
    }
    let config = Config::parse(&plan.config_text, &overrides, &|name| {
        std::env::var(name).ok()
    })
    .map_err(|e| Error::Runner(format!("cell {}: {e}", cell.id())))?;
    Catalog::shipped()
        .and_then(|c| c.check_config(&config, plan.allow_fake))
        .map_err(|e| Error::Runner(format!("cell {}: {e}", cell.id())))?;
    Ok(config)
}

pub(crate) fn block_on<F: std::future::Future>(f: F) -> Result<F::Output> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Error::Runner(format!("runtime: {e}")))?;
    Ok(runtime.block_on(f))
}

/// Runs (or resumes) the matrix. Returns the summary, also written to `summary.json`.
pub fn run(plan: &Plan, factories: &dyn Factories, cancel: Option<&Cancel>) -> Result<Summary> {
    std::fs::create_dir_all(&plan.eval_dir).map_err(|e| io(&plan.eval_dir, e))?;
    let _lock = WriterLock::acquire(&plan.eval_dir)?;
    let m = &plan.matrix.matrix;
    let admissions = plan.admissions.clone().unwrap_or_default();
    // Sets, checked, in matrix order.
    let mut sets = Vec::new();
    for id in &m.sets {
        let set = Set::load(&plan.evalsets_root.join(id))?;
        set.check()?;
        sets.push(set);
    }
    // Tool selections per arm and use case, and the manifests the descriptor binds.
    let mut tool_digests: BTreeMap<String, (String, Option<String>)> = BTreeMap::new();
    let cells = plan.matrix.cells();
    let mut selections: BTreeMap<(String, String), ToolSelection> = BTreeMap::new();
    for set in &sets {
        let use_case = set.manifest.set.use_case.clone();
        for cell in &cells {
            let key = (cell.arm.to_string(), use_case.clone());
            if selections.contains_key(&key) {
                continue;
            }
            let selection = tools_for(&cell.arm, &admissions, &use_case)?;
            if !selection.enabled.is_empty() {
                let config = cell_config(plan, cell, set, &selection)?;
                let registry = factories.tools(&selection, &config)?;
                let locks = factories.lock_digests(&config);
                for manifest in registry.manifests() {
                    let text = serde_json::to_string(manifest)
                        .map_err(|e| Error::Runner(e.to_string()))?;
                    tool_digests.insert(
                        manifest.name.clone(),
                        (text, locks.get(&manifest.name).cloned()),
                    );
                }
            }
            selections.insert(key, selection);
        }
    }
    let descriptor = Descriptor::compute(&Inputs {
        matrix: plan.matrix.clone(),
        matrix_text: plan.matrix_text.clone(),
        config_text: plan.config_text.clone(),
        evalsets_root: plan.evalsets_root.clone(),
        tools: tool_digests
            .into_iter()
            .map(|(name, (manifest, lock))| (name, manifest, lock))
            .collect(),
        selections: selections
            .iter()
            .map(|((arm, use_case), sel)| {
                let tools = sel
                    .enabled
                    .iter()
                    .map(|t| match sel.projections.get(t) {
                        Some(p) => format!("{t}:{p}"),
                        None => t.clone(),
                    })
                    .collect();
                (format!("{arm}@{use_case}"), tools)
            })
            .collect(),
        harness_version: plan.harness_version.clone(),
    })?;
    if plan
        .eval_dir
        .join(crate::descriptor::DESCRIPTOR_FILE)
        .exists()
    {
        Descriptor::check_resume(&plan.eval_dir, &descriptor)?;
    } else {
        descriptor.write(&plan.eval_dir)?;
    }
    let ledger = EvalLedger::open(&plan.eval_dir, m.spend_cap_usd)?;
    let mut summary = Summary {
        summary_version: 1,
        matrix: m.name.clone(),
        matrix_identity: plan.matrix.identity(),
        descriptor_sha256: descriptor.sha256.clone(),
        decisions: Vec::new(),
        completed: 0,
        skipped: 0,
        partial: Vec::new(),
        cancelled: false,
        budget_exhausted: false,
        spent_usd: 0.0,
        cap_usd: m.spend_cap_usd,
        bootstrap: m.gate.as_ref().map(|g| crate::metrics::BootstrapSpec {
            seed: g.seed,
            resamples: g.resamples,
            hypotheses: plan.matrix.hypotheses(),
        }),
    };
    'outer: for set in &sets {
        let set_id = set.manifest.set.id.clone();
        let use_case = set.manifest.set.use_case.clone();
        for (item_id, job) in &set.jobs {
            for cell in &cells {
                let selection = selections
                    .get(&(cell.arm.to_string(), use_case.clone()))
                    .cloned()
                    .unwrap_or_default();
                for repetition in 1..=m.repetitions {
                    let id = format!("{set_id}/{item_id}/{}/rep-{repetition}", cell.id());
                    let rel = decision_dir(cell, &set_id, item_id, repetition);
                    let dir = plan.eval_dir.join(&rel);
                    if dir.join(RESULT_FILE).exists() {
                        let text = std::fs::read_to_string(dir.join(RESULT_FILE))
                            .map_err(|e| io(&dir, e))?;
                        let r: DecisionResult = serde_json::from_str(&text)
                            .map_err(|e| Error::Runner(format!("{}: {e}", dir.display())))?;
                        summary.decisions.push(r);
                        summary.skipped += 1;
                        continue;
                    }
                    if let Some(c) = cancel
                        && c.is_cancelled()
                    {
                        summary.cancelled = true;
                        break 'outer;
                    }
                    let config = cell_config(plan, cell, set, &selection)?;
                    let provider = factories.provider(cell, &config)?;
                    let tools = factories.tools(&selection, &config)?;
                    // The cap must still cover this decision's envelope; when it cannot, the run
                    // stops here rather than recording refusals as results.
                    if let Some(pricing) = provider.pricing() {
                        let envelope = run::envelope(&config, job, &set.dir, &tools, pricing)
                            .map_err(|e| Error::Runner(format!("{id}: {e}")))?;
                        let remaining = Ledger::cap(&ledger)
                            - Ledger::spent(&ledger)
                            - Ledger::reserved(&ledger);
                        if envelope.usd > remaining + 1e-9 {
                            summary.budget_exhausted = true;
                            break 'outer;
                        }
                    }
                    if dir.exists()
                        && std::fs::read_dir(&dir)
                            .map(|mut d| d.next().is_some())
                            .unwrap_or(false)
                    {
                        summary.partial.push(id.clone());
                    }
                    std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
                    ledger.begin(&id);
                    let mut services =
                        Services::new(provider.as_ref(), &tools).with_shared_ledger(Some(&ledger));
                    services.evaluation = true;
                    services.cancel = cancel;
                    let started = Instant::now();
                    let outcome = match block_on(run::check(
                        &config,
                        job,
                        &set.dir,
                        &services,
                        &dir,
                        &plan.harness_version,
                    ))? {
                        Ok(outcome) => outcome,
                        // Cancelled before the decision was allocated: nothing to redo but it.
                        Err(squint_core::Error::Cancelled(_)) => {
                            summary.cancelled = true;
                            break 'outer;
                        }
                        Err(e) => return Err(Error::Runner(format!("{id}: {e}"))),
                    };
                    let wall_ms = started.elapsed().as_millis() as u64;
                    if let Some(c) = cancel
                        && c.is_cancelled()
                    {
                        // The decision ended by cancellation: no result, so it is redone on resume.
                        summary.cancelled = true;
                        break 'outer;
                    }
                    let report: serde_json::Value =
                        std::fs::read_to_string(outcome.dir.join("report.json"))
                            .ok()
                            .and_then(|t| serde_json::from_str(&t).ok())
                            .unwrap_or(serde_json::Value::Null);
                    let error_class = report["runs"][0]["error"]["class"]
                        .as_str()
                        .map(str::to_string);
                    let result = DecisionResult {
                        id: id.clone(),
                        set: set_id.clone(),
                        item: item_id.clone(),
                        cell: cell.id(),
                        repetition,
                        decision_repetition: repetition == 1,
                        verdict: outcome
                            .decision
                            .verdict
                            .map(|v| format!("{v:?}").to_ascii_lowercase()),
                        exit_code: outcome.exit_code,
                        error_class: error_class
                            .or_else(|| outcome.error.as_ref().map(|_| "error".to_string())),
                        error: outcome.error.clone(),
                        cost_usd: report["cost"]["spent_usd"].as_f64().unwrap_or(0.0),
                        wall_ms,
                        dir: relative_dir(&outcome.dir, &plan.eval_dir),
                    };
                    let text = serde_json::to_string_pretty(&result)
                        .map_err(|e| Error::Runner(e.to_string()))?;
                    crate::write_atomic(&dir.join(RESULT_FILE), text.as_bytes())
                        .map_err(|e| io(&dir, e))?;
                    summary.decisions.push(result);
                    summary.completed += 1;
                }
            }
        }
    }
    summary.spent_usd = Ledger::spent(&ledger);
    let text = serde_json::to_string_pretty(&summary).map_err(|e| Error::Runner(e.to_string()))?;
    crate::write_atomic(&plan.eval_dir.join(SUMMARY_FILE), text.as_bytes())
        .map_err(|e| io(&plan.eval_dir, e))?;
    crate::summary::write(&plan.eval_dir, &sets, &summary)?;
    Ok(summary)
}
