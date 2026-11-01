//! `squint serve`: the MCP server over stdio (table-manners 5, ADR 0018), on `rmcp` at revision
//! `2025-11-25` only. Every action in the description is a tool, plus `describe`. Nothing but the
//! protocol goes to stdout.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rmcp::ServiceExt;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
    Implementation, InitializeResult, ListResourceTemplatesResult, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, ProgressNotificationParam, ProtocolVersion,
    ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult, Resource,
    ResourceContents, ResourceTemplate, ServerCapabilities, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::{Map, Value, json};
use squint_core::config::Config;
use squint_core::contract;
use squint_core::ledger::{CapLedger, Ledger};
use squint_core::readiness::{Answers, Inspection};
use squint_core::tool::ToolRegistry;
use table_manners::Description;
use table_manners::images::ImageSlot;
use tokio::sync::Semaphore;

use crate::{EXIT_USAGE, VERSION};

/// How many tools a `tools/list` page holds.
const PAGE: usize = 32;

/// The longest silence on a live call before a heartbeat: half the contract's interval
/// (`contract::PROGRESS_INTERVAL_S`), so a slow notification never makes a client wait longer.
const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(clap::Args)]
pub struct ServeArgs {
    /// Path to the config file; the server checks against it for its whole life. Without it,
    /// squint looks for ./squint.toml, then $XDG_CONFIG_HOME/squint/squint.toml; with none, the
    /// server still starts, and refuses every call that needs one `not_ready`.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// Override one config value: `--set serve.session_budget_usd=5` (repeatable).
    #[arg(long = "set", value_name = "PATH=VALUE")]
    pub overrides: Vec<String>,
    /// Where run directories go; overrides `[output] dir`. An unconfigured server writes nothing.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Scripted envelopes for the fake `fake_regions` tool, for tests and debugging only.
    #[cfg(feature = "testkit")]
    #[arg(long)]
    pub tool_script: Option<PathBuf>,
    /// Scripted fake provider responses for every check, for tests and debugging only.
    #[cfg(feature = "testkit")]
    #[arg(long)]
    pub provider_script: Option<PathBuf>,
}

/// Runs per page of `squint://runs`.
const RUNS_PAGE: usize = 50;

/// A page's cursor: the last run's (started, id), base64url without padding, so it goes into a
/// URI query as is (table-manners 4.3).
fn encode_cursor(ms: u64, id: &str) -> String {
    squint_core::image::base64_encode(format!("{ms}:{id}").as_bytes())
        .trim_end_matches('=')
        .replace('+', "-")
        .replace('/', "_")
}

/// The position a cursor names, or none for one squint did not make.
fn decode_cursor(cursor: &str) -> Option<(u64, String)> {
    let mut standard = cursor.replace('-', "+").replace('_', "/");
    while !standard.len().is_multiple_of(4) {
        standard.push('=');
    }
    let text = String::from_utf8(squint_core::image::base64_decode(&standard).ok()?).ok()?;
    let (ms, id) = text.split_once(':')?;
    Some((ms.parse().ok()?, id.to_string()))
}

/// The request `_meta` key a client's correlation id travels under (table-manners 4.4).
const CLIENT_RUN_META: &str = "table-manners/client_run_id";
/// The notification `_meta` key a run's reference travels under (table-manners 4.3).
const RUN_META: &str = "table-manners/run";

/// A correlation id the contract accepts: `^[A-Za-z0-9._:-]{1,128}$`.
fn valid_client_run_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

/// A random UUID (version 4, RFC 9562), lower case.
fn uuid_v4() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// What a call's job resolves against. Its image paths are absolute (`job_from_call` refuses any
/// other), so this resolves only the config's relative `[rules] extra` files: against the
/// working directory, as `squint check` does.
const JOB_BASE: &str = ".";

pub fn run(args: &ServeArgs) -> i32 {
    let usage = |e: String| {
        eprintln!("squint: {e}");
        EXIT_USAGE
    };
    // Read once, here (table-manners 4.6); a configuration that is missing or does not load is
    // a requirement every action reports, not a reason to refuse to start (3.1, 5).
    #[cfg(feature = "testkit")]
    let scripted = args.provider_script.is_some();
    #[cfg(not(feature = "testkit"))]
    let scripted = false;
    let described = match crate::describe::described(
        Config::open(args.config.as_deref(), &args.overrides),
        scripted,
    ) {
        Ok(described) => described,
        Err(e) => return usage(e.to_string()),
    };
    let config = described.file.config().cloned();
    #[cfg(feature = "testkit")]
    let script = args.tool_script.as_deref();
    #[cfg(not(feature = "testkit"))]
    let script = None;
    // An unconfigured server runs nothing, so it starts no tool pack.
    let tools = match &config {
        Some(config) => crate::tools_for_config(config, script, &Default::default()),
        None => ToolRegistry::new(Vec::new(), &[], &[]).map_err(|e| e.to_string()),
    };
    let tools = match tools {
        Ok(t) => t,
        Err(e) => return usage(e),
    };
    let catalog = match squint_providers::catalog::Catalog::shipped() {
        Ok(c) => c,
        Err(e) => return usage(e.to_string()),
    };
    let out_dir = args
        .out
        .clone()
        .or_else(|| config.as_ref().map(|c| c.output.dir.clone()));
    let serve = config.as_ref().map(|c| c.serve.clone()).unwrap_or_default();
    let server = Server {
        inner: Arc::new(Inner {
            session_id: uuid_v4(),
            session_started_ms: contract::now_ms(),
            claims: Default::default(),
            session: CapLedger::new(serve.session_budget_usd),
            permits: Arc::new(Semaphore::new(serve.concurrency as usize)),
            #[cfg(feature = "testkit")]
            provider_script: args.provider_script.clone(),
            inspection: described.inspection,
            answers: Default::default(),
            description: described.description,
            config,
            tools,
            catalog,
            out_dir,
        }),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return usage(format!("cannot start the runtime: {e}")),
    };
    match runtime.block_on(async {
        let service = server.serve(rmcp::transport::io::stdio()).await?;
        service.waiting().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    }) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("squint: serve: {e}");
            squint_core::run::EXIT_ERROR
        }
    }
}

#[derive(Clone)]
struct Server {
    inner: Arc<Inner>,
}

struct Inner {
    /// None when the server started without a configuration that loads: it then refuses every
    /// call that makes a run (table-manners 3.1) and writes nothing.
    config: Option<Config>,
    /// What the local inspection at start found (table-manners 3.1): with `answers`, every
    /// action's readiness, recomputed on each `describe`.
    inspection: Inspection,
    /// The provider answers this session had, by credential name and model.
    answers: Answers,
    tools: ToolRegistry,
    catalog: squint_providers::catalog::Catalog,
    /// The description at start; its readiness is replaced by the current one on each
    /// `describe`.
    description: Description,
    /// Where runs go: `--out`, else `[output] dir`; none for an unconfigured server without
    /// `--out`, whose runs list is empty.
    out_dir: Option<PathBuf>,
    /// This process's session (table-manners 4.6): a random UUID made at start, fixed for the
    /// process (squint never reloads its config), and when it started.
    session_id: String,
    session_started_ms: u64,
    /// Every `client_run_id` this session has accepted (table-manners 4.4), with its run once one
    /// is allocated: claimed on acceptance, so a reuse is refused even before the first runs.
    claims: std::sync::Mutex<std::collections::HashMap<String, Option<table_manners::RunRef>>>,
    /// The session budget: every decision this process serves reserves against it.
    session: CapLedger,
    /// Calls run under a permit: one at a time unless `[serve] concurrency` allows more.
    permits: Arc<Semaphore>,
    #[cfg(feature = "testkit")]
    provider_script: Option<PathBuf>,
}

impl Server {
    /// The tools this server answers: `describe`, `estimate` and the description's actions.
    fn tools(&self) -> Vec<Tool> {
        let mut tools = vec![Tool::new(
            "describe",
            "The table-manners description document: every action, its inputs, its cost.",
            Arc::new(Map::from_iter([
                ("type".to_string(), json!("object")),
                ("properties".to_string(), json!({})),
                ("additionalProperties".to_string(), json!(false)),
            ])),
        )];
        let use_cases: Vec<String> = self
            .inner
            .description
            .actions
            .iter()
            .map(|a| a.name.replace('_', "-"))
            .collect();
        let (params, slots) =
            contract::estimate_arguments(&use_cases.iter().map(String::as_str).collect::<Vec<_>>());
        for action in &self.inner.description.actions {
            tools.push(Tool::new(
                action.name.clone(),
                action.purpose.clone(),
                Arc::new(input_schema(
                    &action.params_schema,
                    action.images.as_deref().unwrap_or_default(),
                )),
            ));
        }
        tools.push(Tool::new(
            "estimate",
            "The worst-case cost of one check of a use case, against the per-call cap.",
            Arc::new(input_schema(&params, &slots)),
        ));
        tools
    }

    /// The description with every action's readiness as of now: the inspection at start and
    /// the provider answers since (table-manners 3.1). A local computation, never a call.
    fn description(&self) -> Description {
        let readiness = self.inner.inspection.readiness(&self.inner.answers);
        let mut description = self.inner.description.clone();
        for action in &mut description.actions {
            action.readiness = readiness.clone();
        }
        description
    }

    fn describe(&self) -> Result<CallToolResult, ErrorData> {
        let value = serde_json::to_value(self.description())
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let mut result = CallToolResult::success(vec![ContentBlock::text(
            "squint's description document (table-manners)",
        )]);
        result.structured_content = Some(value);
        Ok(result)
    }

    /// `estimate`: the envelope a check of the named use case would reserve, from the catalog's
    /// prices; a run of its own, with no spend.
    fn estimate(
        &self,
        args: Map<String, Value>,
        roots: Option<Vec<PathBuf>>,
        association: &contract::Association,
    ) -> Result<CallToolResult, ErrorData> {
        let use_case = args
            .get("use_case")
            .and_then(Value::as_str)
            .ok_or_else(|| ErrorData::invalid_params("use_case: required", None))?
            .to_string();
        let mut job = match contract::job_from_call(&use_case, &args) {
            Ok(job) => job,
            Err(squint_core::Error::Job(m)) => return Err(ErrorData::invalid_params(m, None)),
            Err(e) => return self.finished("estimate", &args, association, Some(&e), Map::new()),
        };
        let (config, _) = self.configured()?;
        // The estimate reads its images' dimensions, so it answers to the roots a check does.
        if let Err(e) = inside_roots(&mut job, roots.as_deref(), &config.serve.roots) {
            return self.finished("estimate", &args, association, Some(&e), Map::new());
        }
        let mut config = config.clone();
        if let Some(max) = args.get("max_spend_usd") {
            let lowered = max
                .as_f64()
                .ok_or_else(|| squint_core::Error::Config("max_spend_usd: a number".into()))
                .and_then(|m| config.lower_spend_cap(m));
            if let Err(e) = lowered {
                return Err(ErrorData::invalid_params(e.to_string(), None));
            }
        }
        let Some(pricing) = self.inner.catalog.model(&config.provider.model) else {
            let e = squint_core::Error::Config(format!(
                "model {:?} is not in the catalog",
                config.provider.model
            ));
            return self.finished("estimate", &args, association, Some(&e), Map::new());
        };
        match squint_core::run::envelope(
            &config,
            &job,
            Path::new(JOB_BASE),
            &self.inner.tools,
            pricing,
        ) {
            Ok(envelope) => {
                let cap = config.budget.spend_cap_usd;
                let body = json!({
                    "use_case": use_case,
                    "model": config.provider.model,
                    "envelope_usd": contract::usd(envelope.usd),
                    "cap_usd": contract::usd(cap),
                    "fits": envelope.usd <= cap,
                    "decision_runs": envelope.decision_runs,
                });
                self.finished(
                    "estimate",
                    &args,
                    association,
                    None,
                    body.as_object().cloned().unwrap_or_default(),
                )
            }
            Err(e) => self.finished("estimate", &args, association, Some(&e), Map::new()),
        }
    }

    /// The configuration and output directory a call that makes a run needs, or its `not_ready`
    /// refusal (table-manners 3.1): a JSON-RPC error with no run, so nothing is written.
    fn configured(&self) -> Result<(&Config, &Path), ErrorData> {
        match (&self.inner.config, &self.inner.out_dir) {
            (Some(config), Some(out_dir)) => Ok((config, out_dir)),
            _ => Err(not_ready(vec![self.inner.inspection.config.clone()])),
        }
    }

    /// The association a run of this server carries: its session, the request's `client_run_id`
    /// when it sent one, and the action.
    fn association(&self, action: &str, client_run_id: Option<String>) -> contract::Association {
        contract::Association {
            session_id: Some(self.inner.session_id.clone()),
            client_run_id,
            action: Some(action.to_string()),
        }
    }

    /// Claims a request's `client_run_id` for this session (table-manners 4.4): a reused one is
    /// refused with the earlier request's run (none while it has none). The claim lasts while the
    /// guard does, and past it once the request has a run or was cancelled (see [`Claim`]).
    fn claim(
        &self,
        client_run_id: &str,
        cancelled: Box<dyn Fn() -> bool + Send + Sync>,
    ) -> Result<Claim, ErrorData> {
        let mut claims = self
            .inner
            .claims
            .lock()
            .map_err(|_| ErrorData::internal_error("claims poisoned", None))?;
        if let Some(run) = claims.get(client_run_id) {
            let refusal = table_manners::Refusal {
                reason: table_manners::RefusalReason::ClientRunIdReused,
                run: run.clone(),
                requirements: None,
                error: None,
            };
            return Err(ErrorData::invalid_params(
                format!("client_run_id {client_run_id:?} was already used in this session"),
                serde_json::to_value(refusal).ok(),
            ));
        }
        claims.insert(client_run_id.to_string(), None);
        Ok(Claim {
            inner: self.inner.clone(),
            id: client_run_id.to_string(),
            cancelled,
        })
    }

    /// The `squint://session` resource (table-manners 4.6): one observation of the ledger.
    fn session(&self) -> table_manners::Session {
        let (spent, reserved, unresolved) = self.inner.session.snapshot();
        let budget = self.inner.session.cap();
        table_manners::Session {
            contract: table_manners::CONTRACT_VERSION.into(),
            tool: table_manners::Tool {
                name: "squint".into(),
                version: VERSION.into(),
            },
            id: self.inner.session_id.clone(),
            started: contract::rfc3339(self.inner.session_started_ms),
            budget_usd: Some(contract::usd(budget)),
            spent_usd: contract::usd(spent),
            unresolved_usd: contract::usd(unresolved),
            reserved_usd: contract::usd(reserved),
            remaining_usd: Some(contract::usd((budget - spent - reserved).max(0.0))),
        }
    }

    /// One decision of a use case: the call's arguments as a job, every image path inside the
    /// allowed roots, the per-call cap lowered by `max_spend_usd`, the decision reserving against
    /// the session budget; the result is the decision's `run.json`.
    fn check(
        &self,
        action: &str,
        args: Map<String, Value>,
        roots: Option<Vec<PathBuf>>,
        hooks: Hooks,
    ) -> Result<CallToolResult, ErrorData> {
        let use_case = action.replace('_', "-");
        let mut job = match contract::job_from_call(&use_case, &args) {
            Ok(job) => job,
            Err(squint_core::Error::Job(m)) => return Err(ErrorData::invalid_params(m, None)),
            Err(e) => {
                return self.finished(action, &args, &hooks.association, Some(&e), Map::new());
            }
        };
        let (config, out_dir) = self.configured()?;
        if let Err(e) = inside_roots(&mut job, roots.as_deref(), &config.serve.roots) {
            return self.finished(action, &args, &hooks.association, Some(&e), Map::new());
        }
        let mut config = config.clone();
        if let Some(max) = args.get("max_spend_usd") {
            let lowered = max
                .as_f64()
                .ok_or_else(|| squint_core::Error::Config("max_spend_usd: a number".into()))
                .and_then(|m| config.lower_spend_cap(m));
            if let Err(e) = lowered {
                return Err(ErrorData::invalid_params(e.to_string(), None));
            }
        }
        let opts = crate::ProviderOptions {
            replay: None,
            record: None,
            #[cfg(feature = "testkit")]
            provider_script: self.inner.provider_script.as_deref(),
        };
        let provider = match crate::provider_for(&mut config, &opts) {
            Ok(p) => p,
            Err(m) => {
                let e = squint_core::Error::Auth(m);
                return self.finished(action, &args, &hooks.association, Some(&e), Map::new());
            }
        };
        // The decision's directory once it exists: an error after that is answered with the
        // decision's own run, never a second one.
        let allocated = Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let told = allocated.clone();
        let server = self.clone();
        let association = hooks.association.clone();
        let progress = hooks.progress.clone();
        // Each settled answer goes into the session's readiness, under the credential this
        // profile reads and the model it calls.
        let session = self.inner.clone();
        let credential = self
            .inner
            .inspection
            .credential
            .as_ref()
            .map(|c| c.name.clone());
        let model = config.provider.model.clone();
        let mut services = squint_core::run::Services::new(provider.as_ref(), &self.inner.tools)
            .with_answered(Arc::new(move |answer| {
                session
                    .answers
                    .record(credential.as_deref(), &model, answer);
            }))
            .with_association(hooks.association.clone())
            .with_shared_ledger(Some(&self.inner.session))
            .with_cancel(&hooks.cancel)
            .with_allocated(Arc::new(move |dir: &Path| {
                if let Ok(mut slot) = told.lock() {
                    *slot = Some(dir.to_path_buf());
                }
                // From here every progress notification names the run (table-manners 4.3).
                let run = server.allocated(dir, &association);
                if let Some(progress) = &progress {
                    let _ = progress.send(Progress::Run(run));
                }
            }));
        if let Some(observer) = hooks.observer {
            services = services.with_observer(observer);
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let outcome = runtime.block_on(squint_core::run::check(
            &config,
            &job,
            Path::new(JOB_BASE),
            &services,
            out_dir,
            VERSION,
        ));
        let decided = allocated.lock().ok().and_then(|slot| slot.clone());
        match (outcome, decided) {
            (Ok(o), _) => decision_result(&o.dir),
            (Err(_), Some(dir)) if dir.join(contract::RUN_FILE).exists() => decision_result(&dir),
            // Cancelled before the decision was allocated: no run (table-manners 4.2).
            (Err(squint_core::Error::Cancelled(m)), None) => {
                Err(ErrorData::internal_error(m, None))
            }
            (Err(e), _) => self.finished(action, &args, &hooks.association, Some(&e), Map::new()),
        }
    }

    /// A resource's text: the runs list, or one run's `run.json`, report or journal.
    fn resource(&self, uri: &str) -> Option<String> {
        if uri == "squint://session" {
            return serde_json::to_string_pretty(&self.session()).ok();
        }
        if uri == "squint://runs" {
            return serde_json::to_string_pretty(&self.runs(None)).ok();
        }
        if let Some(cursor) = uri.strip_prefix("squint://runs?cursor=") {
            let after = decode_cursor(cursor)?;
            return serde_json::to_string_pretty(&self.runs(Some(&after))).ok();
        }
        let (id, what) = uri.strip_prefix("squint://run/")?.split_once('/')?;
        let valid = !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return None;
        }
        let dir = self.inner.out_dir.as_ref()?.join(id);
        match what {
            "run.json" => std::fs::read_to_string(dir.join(contract::RUN_FILE)).ok(),
            "report.json" => std::fs::read_to_string(dir.join("report.json")).ok(),
            // Every run that exists has a journal: a decision's runs' lines in order, and an empty
            // one for a run with no decision journal (an `estimate`, a call refused before its
            // first run). A run that does not exist has none.
            "journal" if dir.is_dir() => {
                let mut text = String::new();
                for n in 1.. {
                    let Ok(part) =
                        std::fs::read_to_string(dir.join(format!("run-{n}/journal.jsonl")))
                    else {
                        break;
                    };
                    text.push_str(&part);
                }
                Some(text)
            }
            _ => None,
        }
    }

    /// One page of the runs under the output directory (table-manners 4.3): newest first by
    /// (started, id), at most [`RUNS_PAGE`], after `after` when a cursor names one. Directory
    /// names carry the start time, so only the page's runs are opened: a finished one by its
    /// `run.json`, one still going as `running`.
    fn runs(&self, after: Option<&(u64, String)>) -> table_manners::RunsList {
        let mut all: Vec<(u64, String, PathBuf)> = self
            .inner
            .out_dir
            .iter()
            .flat_map(std::fs::read_dir)
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let id = entry.file_name().to_string_lossy().into_owned();
                // `<action>-<unix ms>-<pid>[-n]`: the milliseconds order the list.
                let ms: u64 = id.split('-').find_map(|part| part.parse().ok())?;
                Some((ms, id, entry.path()))
            })
            .filter(|(ms, id, _)| after.is_none_or(|(a_ms, a_id)| (*ms, id) < (*a_ms, a_id)))
            .collect();
        all.sort_by(|a, b| (b.0, &b.1).cmp(&(a.0, &a.1)));
        let more = all.len() > RUNS_PAGE;
        all.truncate(RUNS_PAGE);
        let next_cursor = more
            .then(|| all.last().map(|(ms, id, _)| encode_cursor(*ms, id)))
            .flatten();
        let runs = all
            .into_iter()
            .map(|(ms, id, dir)| {
                match std::fs::read_to_string(dir.join(contract::RUN_FILE))
                    .ok()
                    .and_then(|t| serde_json::from_str::<table_manners::RunResult>(&t).ok())
                {
                    Some(r) => table_manners::RunSummary {
                        id: id.clone(),
                        action: r.action,
                        status: match r.status {
                            table_manners::Status::Completed => table_manners::RunState::Completed,
                            table_manners::Status::Cancelled => table_manners::RunState::Cancelled,
                            table_manners::Status::Error => table_manners::RunState::Error,
                            table_manners::Status::Invalid => table_manners::RunState::Invalid,
                        },
                        started: r.started,
                        resource: r.run.resource,
                        session_id: r.run.session_id,
                        client_run_id: r.run.client_run_id,
                    },
                    None => {
                        let association = contract::Association::read(&dir);
                        table_manners::RunSummary {
                            action: association
                                .action
                                .clone()
                                .unwrap_or_else(|| running_action(&dir, &id)),
                            status: table_manners::RunState::Running,
                            started: contract::rfc3339(ms),
                            resource: format!("squint://run/{id}/{}", contract::RUN_FILE),
                            id: id.clone(),
                            session_id: association.session_id,
                            client_run_id: association.client_run_id,
                        }
                    }
                }
            })
            .collect();
        table_manners::RunsList {
            contract: table_manners::CONTRACT_VERSION.into(),
            tool: table_manners::Tool {
                name: "squint".into(),
                version: VERSION.into(),
            },
            runs,
            next_cursor,
        }
    }

    /// Writes an action's run directory with its `run.json` and returns the result: `isError`
    /// for `invalid` and `error`.
    fn finished(
        &self,
        action: &str,
        args: &Map<String, Value>,
        association: &contract::Association,
        error: Option<&squint_core::Error>,
        body: Map<String, Value>,
    ) -> Result<CallToolResult, ErrorData> {
        let (_, out_dir) = self.configured()?;
        let started = contract::now_ms();
        let dir = new_run_dir(out_dir, action, started)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        self.allocated(&dir, association);
        let result = contract::action_result(
            &dir,
            VERSION,
            action,
            association,
            started,
            &Value::Object(args.clone()),
            error,
            body,
        );
        contract::write_run_result(&dir, &result)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(tool_result(&result))
    }
}

impl Server {
    /// A run's directory exists: its reference, recorded against the request's claim.
    fn allocated(&self, dir: &Path, association: &contract::Association) -> table_manners::RunRef {
        let id = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let run = table_manners::RunRef {
            resource: format!("squint://run/{id}/{}", contract::RUN_FILE),
            id,
            session_id: association.session_id.clone(),
            client_run_id: association.client_run_id.clone(),
        };
        if let (Some(client), Ok(mut claims)) =
            (&association.client_run_id, self.inner.claims.lock())
        {
            claims.insert(client.clone(), Some(run.clone()));
        }
        run
    }
}

/// The refusal of a call that needs requirements that are `missing` or `invalid` (table-manners
/// 3.1): `-32602` with `{"reason": "not_ready", "run": null, "requirements": [...]}`, before any
/// run, so it writes nothing, spends nothing and releases its `client_run_id` (4.4).
fn not_ready(requirements: Vec<table_manners::Requirement>) -> ErrorData {
    let names: Vec<String> = requirements
        .iter()
        .map(|r| format!("{} {}", r.name, word_of(&r.state)))
        .collect();
    let refusal = table_manners::Refusal {
        reason: table_manners::RefusalReason::NotReady,
        run: None,
        requirements: Some(requirements),
        error: None,
    };
    ErrorData::invalid_params(
        format!(
            "not ready: {}; call describe for what to set up",
            names.join(", ")
        ),
        serde_json::to_value(refusal).ok(),
    )
}

/// A requirement state's word.
fn word_of(state: &table_manners::RequirementState) -> &'static str {
    use table_manners::RequirementState as S;
    match state {
        S::Missing => "missing",
        S::Present => "present",
        S::Valid => "valid",
        S::Invalid => "invalid",
        S::Unknown => "unknown",
    }
}

/// A request's hold on its `client_run_id` (table-manners 4.4, squint's M8 review S3). Dropped
/// when the request ends: a request that got a run keeps the id (the claim names the run), and so
/// does one cancelled before allocation (the client retries under a new id); any other request
/// that ends with no run (a JSON-RPC error: arguments refused, a failed `roots/list`, a server
/// error) releases it, so the corrected request may use it.
struct Claim {
    inner: Arc<Inner>,
    id: String,
    /// Whether the request was cancelled (its MCP cancellation token).
    cancelled: Box<dyn Fn() -> bool + Send + Sync>,
}

impl Drop for Claim {
    fn drop(&mut self) {
        if (self.cancelled)() {
            return;
        }
        if let Ok(mut claims) = self.inner.claims.lock()
            && matches!(claims.get(&self.id), Some(None))
        {
            claims.remove(&self.id);
        }
    }
}

/// What a check is given by the call around it: the flag a client's cancellation sets, the
/// observer that turns journal lines into progress, where the run's reference goes once it is
/// allocated, and the run's association.
struct Hooks {
    cancel: Arc<squint_core::run::Cancel>,
    observer: Option<squint_core::run::Observer>,
    progress: Option<tokio::sync::mpsc::UnboundedSender<Progress>>,
    association: contract::Association,
}

/// What the progress task hears: the run, once allocated, and each journal line.
enum Progress {
    Run(table_manners::RunRef),
    Line(PathBuf, String),
}

/// Sends progress for one call until its senders go away: a notification per journal line and
/// a heartbeat after every [`HEARTBEAT`] of silence, each carrying the run's reference in
/// `_meta["table-manners/run"]` once the run is allocated (table-manners 4.3). `progress` is
/// elapsed seconds, nudged up so it never repeats (4.1).
async fn send_progress(
    peer: rmcp::Peer<RoleServer>,
    token: rmcp::model::ProgressToken,
    mut events: tokio::sync::mpsc::UnboundedReceiver<Progress>,
) {
    let started = std::time::Instant::now();
    let mut last = 0.0_f64;
    let mut run: Option<table_manners::RunRef> = None;
    loop {
        let message = match tokio::time::timeout(HEARTBEAT, events.recv()).await {
            Ok(Some(Progress::Run(allocated))) => {
                run = Some(allocated);
                continue;
            }
            Ok(Some(Progress::Line(run_dir, event))) => {
                let attempt = run_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                format!("{attempt}: {event}")
            }
            Ok(None) => return,
            Err(_) => format!("waiting, {} s", started.elapsed().as_secs()),
        };
        let mut progress = started.elapsed().as_secs_f64();
        if progress <= last {
            progress = last + 0.001;
        }
        last = progress;
        let mut param = ProgressNotificationParam::new(token.clone(), progress);
        param.message = Some(message);
        if let Some(run) = run.as_ref().and_then(|r| serde_json::to_value(r).ok()) {
            let mut meta = rmcp::model::NotificationMetaObject::new();
            meta.0.0.insert(RUN_META.into(), run);
            param.meta = Some(meta);
        }
        if peer.notify_progress(param).await.is_err() {
            return;
        }
    }
}

/// The action of a run still going that has no association (one made outside this server): a
/// decision's use case from its header, as `describe` names it; else the directory name's
/// leading word.
fn running_action(dir: &Path, id: &str) -> String {
    std::fs::read_to_string(dir.join(squint_core::report::HEADER_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|header| header["use_case"].as_str().map(|u| u.replace('-', "_")))
        .unwrap_or_else(|| id.split('-').next().unwrap_or_default().to_string())
}

/// The local path of a `file:` URL (RFC 8089): an empty or `localhost` host, the path
/// percent-decoded. Anything else names no local directory and is dropped, so it admits nothing.
fn file_url_path(uri: &str) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let rest = uri.strip_prefix("file://")?;
    let path = match rest.find('/') {
        Some(0) => rest,
        Some(i) if rest[..i].eq_ignore_ascii_case("localhost") => &rest[i..],
        _ => return None,
    };
    let mut bytes = Vec::with_capacity(path.len());
    let mut raw = path.bytes();
    while let Some(b) = raw.next() {
        if b == b'%' {
            let hex = [raw.next()?, raw.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

/// Every image path of `job` inside the client's roots (when the client offers roots) and inside
/// the config's `[serve] roots` (when it names any): an image elsewhere is `input`. With neither,
/// no path is allowed (closed by default, table-manners 10.1); inline images need no roots. Each path is
/// replaced by the real one it was checked as, so a symlink swapped afterwards cannot lead out. A
/// path that does not resolve is refused in the words of one outside, so a refusal says nothing
/// about what exists.
fn inside_roots(
    job: &mut squint_core::job::Job,
    client: Option<&[PathBuf]>,
    config: &[PathBuf],
) -> squint_core::Result<()> {
    let within = |path: &Path, roots: &[PathBuf]| {
        roots
            .iter()
            .any(|root| root.canonicalize().is_ok_and(|r| path.starts_with(r)))
    };
    for image in &mut job.images {
        let squint_core::job::ImageSource::Path { path, .. } = &mut image.source else {
            continue;
        };
        let refused = |which: &str| {
            squint_core::Error::Input(format!(
                "{}: {} is not a readable file inside {which}",
                image.id,
                path.display()
            ))
        };
        if client.is_none() && config.is_empty() {
            return Err(squint_core::Error::Input(format!(
                "{}: {} is refused: neither the client nor [serve] roots names a directory squint \
                 may read; send the image inline or configure roots",
                image.id,
                path.display()
            )));
        }
        let real = path.canonicalize();
        if client.is_some_and(|roots| !real.as_ref().is_ok_and(|r| within(r, roots))) {
            return Err(refused("the client's roots"));
        }
        if !config.is_empty() && !real.as_ref().is_ok_and(|r| within(r, config)) {
            return Err(refused("[serve] roots"));
        }
        match real {
            Ok(real) => *path = real,
            Err(e) => {
                return Err(squint_core::Error::Input(format!(
                    "{}: {}: {e}",
                    image.id,
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

/// A decision's `run.json` as the call's result.
fn decision_result(dir: &Path) -> Result<CallToolResult, ErrorData> {
    let text = std::fs::read_to_string(dir.join(contract::RUN_FILE))
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    let result: table_manners::RunResult =
        serde_json::from_str(&text).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    Ok(tool_result(&result))
}

/// The contract's result as an MCP tool result: the header and body as structured content, a
/// line of text for clients that show only text, `isError` for `invalid` and `error`.
fn tool_result(result: &table_manners::RunResult) -> CallToolResult {
    let summary = match &result.error {
        Some(e) => format!("{} {}: {}", result.action, word(&result.status), e.reason),
        None => format!("{} {}", result.action, word(&result.status)),
    };
    let mut out = CallToolResult::success(vec![ContentBlock::text(summary)]);
    out.structured_content = serde_json::to_value(result).ok();
    out.is_error = Some(matches!(
        result.status,
        table_manners::Status::Invalid | table_manners::Status::Error
    ));
    out
}

fn word(status: &table_manners::Status) -> &'static str {
    match status {
        table_manners::Status::Completed => "completed",
        table_manners::Status::Cancelled => "cancelled",
        table_manners::Status::Error => "error",
        table_manners::Status::Invalid => "invalid",
    }
}

/// A fresh run directory, `<action>-<started ms>-<pid>[-n]`, absolute: named for the start its
/// `run.json` records, so the runs list's order and its times agree.
fn new_run_dir(out_dir: &Path, action: &str, started_ms: u64) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let out_dir = std::path::absolute(out_dir)?;
    let stem = format!("{action}-{started_ms}-{}", std::process::id());
    let mut n = 0u32;
    loop {
        let name = if n == 0 {
            stem.clone()
        } else {
            format!("{stem}-{n}")
        };
        let dir = out_dir.join(name);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e),
        }
    }
}

/// A tool's MCP input schema: its parameters, its image slots as images in (`references` a list
/// of them), and the locator and image-block choices.
fn input_schema(params_schema: &Value, slots: &[ImageSlot]) -> Map<String, Value> {
    let image_in = json!({
        "type": "object",
        "description": "An image by absolute path or inline bytes, with an optional expected reference (table-manners 10.1).",
        "properties": {
            "path": { "type": "string" },
            "inline": {
                "type": "object",
                "properties": { "media_type": { "type": "string" }, "data": { "type": "string" } },
                "required": ["media_type", "data"]
            },
            "expect": { "type": "object" }
        }
    });
    let mut properties = params_schema["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut required: Vec<Value> = params_schema["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for slot in slots {
        let schema = if slot.slot == "references" {
            json!({ "type": "array", "items": image_in })
        } else {
            image_in.clone()
        };
        properties.insert(slot.slot.clone(), schema);
        if slot.required {
            required.push(json!(slot.slot));
        }
    }
    properties.insert(
        "return_images".into(),
        json!({ "enum": ["path", "inline", "resource"], "description": "How returned images are located; path by default." }),
    );
    properties.insert(
        "image_blocks".into(),
        json!({ "type": "boolean", "description": "Also send returned images as MCP image blocks." }),
    );
    Map::from_iter([
        ("type".to_string(), json!("object")),
        ("properties".to_string(), Value::Object(properties)),
        ("required".to_string(), Value::Array(required)),
        ("additionalProperties".to_string(), json!(false)),
        // The definitions the parameters refer to, kept at this schema's root (squint issue #2).
        ("$defs".to_string(), params_schema["$defs"].clone()),
    ])
}

impl ServerHandler for Server {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
            .with_server_info(Implementation::new("squint", VERSION))
            .with_instructions(
                "squint checks an image against what was asked of it. Call describe for the actions; estimate before a paid check.",
            )
    }

    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2025_11_25])
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let start: usize = match request.and_then(|r| r.cursor) {
            Some(c) => c
                .parse()
                .map_err(|_| ErrorData::invalid_params(format!("bad cursor {c}"), None))?,
            None => 0,
        };
        let all = self.tools();
        let end = start.saturating_add(PAGE).min(all.len());
        Ok(ListToolsResult {
            tools: all.get(start..end).unwrap_or_default().to_vec(),
            next_cursor: (end < all.len()).then(|| end.to_string()),
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = request.arguments.clone().unwrap_or_default();
        let server = self.clone();
        let name = request.name.to_string();
        let is_check = name != "estimate"
            && self
                .inner
                .description
                .actions
                .iter()
                .any(|a| a.name == name);
        let makes_run = is_check || name == "estimate";
        // The request's correlation id (table-manners 4.4): checked, then claimed for this
        // session before anything waits, so two submissions of one id execute at most once.
        let client_run_id = match context.meta.get(CLIENT_RUN_META) {
            None => None,
            Some(Value::String(id)) if valid_client_run_id(id) => Some(id.clone()),
            Some(other) => {
                return Err(ErrorData::invalid_params(
                    format!(
                        "_meta {CLIENT_RUN_META:?} is {other}; it must match ^[A-Za-z0-9._:-]{{1,128}}$"
                    ),
                    None,
                ));
            }
        };
        // Held until this request ends, on every path out of it, `?` included.
        let _claim = match (&client_run_id, makes_run) {
            (Some(id), true) => {
                let ct = context.ct.clone();
                Some(self.claim(id, Box::new(move || ct.is_cancelled()))?)
            }
            _ => None,
        };
        let association = self.association(&name, client_run_id);
        // A call that makes a run needs the configuration; without one it is refused here,
        // before roots, the permit or any directory (table-manners 3.1).
        if makes_run {
            self.configured()?;
        }
        // A client that offers roots is asked for them on every check: they may have changed.
        // Roots are part of revision 2025-11-25, the only one squint serves; SEP-2577 deprecates
        // them in a later revision, which the contract must answer before squint moves (v0.2).
        #[allow(deprecated)]
        let roots = if makes_run
            && context
                .peer
                .peer_info()
                .is_some_and(|info| info.capabilities.roots.is_some())
        {
            let listed = context
                .peer
                .list_roots()
                .await
                .map_err(|e| ErrorData::internal_error(format!("roots/list: {e}"), None))?;
            Some(
                listed
                    .roots
                    .iter()
                    .filter_map(|r| file_url_path(&r.uri))
                    .collect(),
            )
        } else {
            None
        };
        let cancel = Arc::new(squint_core::run::Cancel::new());
        // Progress starts before any wait, so a call queued for the permit beats too.
        let (sender, progress) = match context.meta.get_progress_token().filter(|_| is_check) {
            Some(token) => {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let task = tokio::spawn(send_progress(context.peer.clone(), token, rx));
                (Some(tx), Some(task))
            }
            None => (None, None),
        };
        let observer = sender.clone().map(|tx| -> squint_core::run::Observer {
            Arc::new(move |run_dir: &Path, event: &str| {
                let _ = tx.send(Progress::Line(run_dir.to_path_buf(), event.to_string()));
            })
        });
        // Only a check takes the execution permit: `describe` and `estimate` spend nothing and
        // stay responsive while a check runs (table-manners C4 review 9). The wait itself is
        // cancellable, and a call cancelled before it holds the permit gets no run (review 10).
        let permit = if is_check {
            let acquired = tokio::select! {
                acquired = self.inner.permits.clone().acquire_owned() => Some(acquired),
                () = context.ct.cancelled() => None,
            };
            match acquired {
                Some(Ok(permit)) if !context.ct.is_cancelled() => Some(permit),
                Some(Err(e)) => return Err(ErrorData::internal_error(e.to_string(), None)),
                _ => {
                    drop((observer, sender));
                    if let Some(progress) = progress {
                        let _ = progress.await;
                    }
                    // rmcp sends nothing for a cancelled request; this never reaches the client.
                    return Err(ErrorData::internal_error(
                        "cancelled before a run was allocated",
                        None,
                    ));
                }
            }
        } else {
            None
        };
        let hooks = Hooks {
            cancel: cancel.clone(),
            observer,
            progress: sender,
            association: association.clone(),
        };
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            match name.as_str() {
                "describe" => server.describe(),
                "estimate" => server.estimate(args, roots, &association),
                action if is_check => server.check(action, args, roots, hooks),
                other => Err(ErrorData::invalid_params(
                    format!("no tool {other:?}; call describe for the actions"),
                    None,
                )),
            }
        });
        tokio::pin!(task);
        // A cancellation stops the run's paid work; the run then writes its `cancelled` run.json,
        // and rmcp sends no response to a cancelled request (table-manners 4.2).
        let joined = tokio::select! {
            joined = &mut task => joined,
            () = context.ct.cancelled() => {
                cancel.cancel();
                (&mut task).await
            }
        };
        if let Some(progress) = progress {
            let _ = progress.await;
        }
        let result = joined
            .map_err(|e| ErrorData::internal_error(format!("the call panicked: {e}"), None))??;
        Ok(result.into())
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let mut runs = Resource::new("squint://runs", "runs");
        runs.description = Some("Recent runs, newest first (table-manners 4.3).".into());
        runs.mime_type = Some("application/json".into());
        let mut session = Resource::new("squint://session", "session");
        session.description = Some(
            "This server's session: its id and budget, spent and reserved (table-manners 4.6)."
                .into(),
        );
        session.mime_type = Some("application/json".into());
        Ok(ListResourcesResult::with_all_items(vec![runs, session]))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let templates = [
            ("squint://runs{?cursor}", "runs page", "application/json"),
            ("squint://run/{id}/run.json", "run", "application/json"),
            (
                "squint://run/{id}/report.json",
                "report",
                "application/json",
            ),
            (
                "squint://run/{id}/journal",
                "journal",
                "application/x-ndjson",
            ),
        ]
        .into_iter()
        .map(|(uri, name, mime)| {
            let mut t = ResourceTemplate::new(uri, name);
            t.mime_type = Some(mime.into());
            t
        })
        .collect();
        Ok(ListResourceTemplatesResult::with_all_items(templates))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.clone();
        let server = self.clone();
        let lookup = uri.clone();
        let text = tokio::task::spawn_blocking(move || server.resource(&lookup))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
            .ok_or_else(|| ErrorData::resource_not_found(format!("no resource {uri}"), None))?;
        Ok(ReadResourceResult::new(vec![ResourceContents::text(text, uri)]).into())
    }
}
