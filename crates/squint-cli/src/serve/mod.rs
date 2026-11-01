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
use squint_core::tool::ToolRegistry;
use table_manners::Description;
use tokio::sync::Semaphore;

use crate::{EXIT_USAGE, VERSION};

/// How many tools a `tools/list` page holds.
const PAGE: usize = 32;

/// The longest silence on a live call before a heartbeat: half the contract's interval
/// (`contract::PROGRESS_INTERVAL_S`), so a slow notification never makes a client wait longer.
const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(clap::Args)]
pub struct ServeArgs {
    /// Path to the config file; the server checks against it for its whole life.
    #[arg(long)]
    pub config: PathBuf,
    /// Override one config value: `--set serve.session_budget_usd=5` (repeatable).
    #[arg(long = "set", value_name = "PATH=VALUE")]
    pub overrides: Vec<String>,
    /// Where run directories go; overrides `[output] dir`.
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

/// What a call's job resolves against. Its image paths are absolute (`job_from_call` refuses any
/// other), so this resolves only the config's relative `[rules] extra` files: against the
/// working directory, as `squint check` does.
const JOB_BASE: &str = ".";

pub fn run(args: &ServeArgs) -> i32 {
    let usage = |e: String| {
        eprintln!("squint: {e}");
        EXIT_USAGE
    };
    let config = match Config::load_with_overrides(&args.config, &args.overrides) {
        Ok(c) => c,
        Err(e) => return usage(e.to_string()),
    };
    #[cfg(feature = "testkit")]
    let script = args.tool_script.as_deref();
    #[cfg(not(feature = "testkit"))]
    let script = None;
    let tools = match crate::tools_for_config(&config, script, &Default::default()) {
        Ok(t) => t,
        Err(e) => return usage(e),
    };
    let catalog = match squint_providers::catalog::Catalog::shipped() {
        Ok(c) => c,
        Err(e) => return usage(e.to_string()),
    };
    let needs_env: Vec<String> = catalog
        .profile(&config.provider.profile)
        .map(|p| p.auth.env.clone())
        .into_iter()
        .collect();
    let description = match contract::description(&config, VERSION, &needs_env) {
        Ok(d) => d,
        Err(e) => return usage(e.to_string()),
    };
    let out_dir = args
        .out
        .clone()
        .unwrap_or_else(|| config.output.dir.clone());
    let server = Server {
        inner: Arc::new(Inner {
            session: CapLedger::new(config.serve.session_budget_usd),
            permits: Arc::new(Semaphore::new(config.serve.concurrency as usize)),
            #[cfg(feature = "testkit")]
            provider_script: args.provider_script.clone(),
            config,
            tools,
            catalog,
            description,
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
    config: Config,
    tools: ToolRegistry,
    catalog: squint_providers::catalog::Catalog,
    description: Description,
    out_dir: PathBuf,
    /// The session budget: every decision this process serves reserves against it.
    session: CapLedger,
    /// Calls run under a permit: one at a time unless `[serve] concurrency` allows more.
    permits: Arc<Semaphore>,
    #[cfg(feature = "testkit")]
    provider_script: Option<PathBuf>,
}

impl Server {
    /// The tools this server answers: `describe` and the description's actions it serves.
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
        for action in &self.inner.description.actions {
            tools.push(Tool::new(
                action.name.clone(),
                action.purpose.clone(),
                Arc::new(input_schema(action)),
            ));
        }
        tools
    }

    fn describe(&self) -> Result<CallToolResult, ErrorData> {
        let value = serde_json::to_value(&self.inner.description)
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
    ) -> Result<CallToolResult, ErrorData> {
        let use_case = args
            .get("use_case")
            .and_then(Value::as_str)
            .ok_or_else(|| ErrorData::invalid_params("use_case: required", None))?
            .to_string();
        let mut job = match contract::job_from_call(&use_case, &args) {
            Ok(job) => job,
            Err(squint_core::Error::Job(m)) => return Err(ErrorData::invalid_params(m, None)),
            Err(e) => return self.finished("estimate", &args, Some(&e), Map::new()),
        };
        // The estimate reads its images' dimensions, so it answers to the roots a check does.
        if let Err(e) = inside_roots(&mut job, roots.as_deref(), &self.inner.config.serve.roots) {
            return self.finished("estimate", &args, Some(&e), Map::new());
        }
        let mut config = self.inner.config.clone();
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
            return self.finished("estimate", &args, Some(&e), Map::new());
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
                    "envelope_usd": envelope.usd,
                    "cap_usd": cap,
                    "fits": envelope.usd <= cap,
                    "decision_runs": envelope.decision_runs,
                    "session_budget_left_usd": self.session_left(),
                });
                self.finished(
                    "estimate",
                    &args,
                    None,
                    body.as_object().cloned().unwrap_or_default(),
                )
            }
            Err(e) => self.finished("estimate", &args, Some(&e), Map::new()),
        }
    }

    /// What the session budget has left: the cap less what is spent and reserved.
    fn session_left(&self) -> f64 {
        let s = &self.inner.session;
        (s.cap() - s.spent() - s.reserved()).max(0.0)
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
            Err(e) => return self.finished(action, &args, Some(&e), Map::new()),
        };
        if let Err(e) = inside_roots(&mut job, roots.as_deref(), &self.inner.config.serve.roots) {
            return self.finished(action, &args, Some(&e), Map::new());
        }
        let mut config = self.inner.config.clone();
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
                return self.finished(action, &args, Some(&e), Map::new());
            }
        };
        // The decision's directory once it exists: an error after that is answered with the
        // decision's own run, never a second one.
        let allocated = Arc::new(std::sync::Mutex::new(None::<PathBuf>));
        let told = allocated.clone();
        let mut services = squint_core::run::Services::new(provider.as_ref(), &self.inner.tools)
            .with_shared_ledger(Some(&self.inner.session))
            .with_cancel(&hooks.cancel)
            .with_allocated(Arc::new(move |dir: &Path| {
                if let Ok(mut slot) = told.lock() {
                    *slot = Some(dir.to_path_buf());
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
            &self.inner.out_dir,
            VERSION,
        ));
        let decided = allocated.lock().ok().and_then(|slot| slot.clone());
        match (outcome, decided) {
            (Ok(o), _) => decision_result(&o.dir),
            (Err(_), Some(dir)) if dir.join(contract::RUN_FILE).exists() => decision_result(&dir),
            (Err(e), _) => self.finished(action, &args, Some(&e), Map::new()),
        }
    }

    /// A resource's text: the runs list, or one run's `run.json`, report or journal.
    fn resource(&self, uri: &str) -> Option<String> {
        if uri == "squint://runs" {
            return serde_json::to_string_pretty(&self.runs()).ok();
        }
        let (id, what) = uri.strip_prefix("squint://run/")?.split_once('/')?;
        let valid = !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !valid {
            return None;
        }
        let dir = self.inner.out_dir.join(id);
        match what {
            "run.json" => std::fs::read_to_string(dir.join(contract::RUN_FILE)).ok(),
            "report.json" => std::fs::read_to_string(dir.join("report.json")).ok(),
            "journal" => {
                let mut text = String::new();
                for n in 1.. {
                    let Ok(part) =
                        std::fs::read_to_string(dir.join(format!("run-{n}/journal.jsonl")))
                    else {
                        break;
                    };
                    text.push_str(&part);
                }
                (!text.is_empty()).then_some(text)
            }
            _ => None,
        }
    }

    /// Every run directory under the output directory, newest first: a finished one by its
    /// `run.json`, one still going as `running` (table-manners 4.3).
    fn runs(&self) -> table_manners::RunsList {
        let mut runs: Vec<(u64, table_manners::RunSummary)> =
            std::fs::read_dir(&self.inner.out_dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let id = entry.file_name().to_string_lossy().into_owned();
                    // `<action>-<unix ms>-<pid>[-n]`: the milliseconds order the list.
                    let ms: u64 = id.split('-').find_map(|part| part.parse().ok())?;
                    let dir = entry.path();
                    let summary = match std::fs::read_to_string(dir.join(contract::RUN_FILE))
                        .ok()
                        .and_then(|t| serde_json::from_str::<table_manners::RunResult>(&t).ok())
                    {
                        Some(r) => table_manners::RunSummary {
                            id: id.clone(),
                            action: r.action,
                            status: match r.status {
                                table_manners::Status::Completed => {
                                    table_manners::RunState::Completed
                                }
                                table_manners::Status::Cancelled => {
                                    table_manners::RunState::Cancelled
                                }
                                table_manners::Status::Error => table_manners::RunState::Error,
                                table_manners::Status::Invalid => table_manners::RunState::Invalid,
                            },
                            started: r.started,
                            resource: r.run.resource,
                        },
                        None => table_manners::RunSummary {
                            action: id.split('-').next().unwrap_or("check").to_string(),
                            status: table_manners::RunState::Running,
                            started: contract::rfc3339(ms),
                            resource: format!("squint://run/{id}/{}", contract::RUN_FILE),
                            id: id.clone(),
                        },
                    };
                    Some((ms, summary))
                })
                .collect();
        runs.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.id.cmp(&a.1.id)));
        table_manners::RunsList {
            contract: table_manners::CONTRACT_VERSION.into(),
            tool: table_manners::Tool {
                name: "squint".into(),
                version: VERSION.into(),
            },
            runs: runs.into_iter().map(|(_, r)| r).collect(),
        }
    }

    /// Writes an action's run directory with its `run.json` and returns the result: `isError`
    /// for `invalid` and `error`.
    fn finished(
        &self,
        action: &str,
        args: &Map<String, Value>,
        error: Option<&squint_core::Error>,
        body: Map<String, Value>,
    ) -> Result<CallToolResult, ErrorData> {
        let started = contract::now_ms();
        let dir = new_run_dir(&self.inner.out_dir, action)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let result = contract::action_result(
            &dir,
            VERSION,
            action,
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

/// What a check is given by the call around it: the flag a client's cancellation sets, and the
/// observer that turns journal lines into progress.
struct Hooks {
    cancel: Arc<squint_core::run::Cancel>,
    observer: Option<squint_core::run::Observer>,
}

/// Sends progress for one call until the run's observer goes away: a notification per journal
/// line, the first naming the run's resource, and a heartbeat after every [`HEARTBEAT`] of
/// silence. `progress` is elapsed seconds, nudged up so it never repeats (table-manners 4.1).
async fn send_progress(
    peer: rmcp::Peer<RoleServer>,
    token: rmcp::model::ProgressToken,
    mut lines: tokio::sync::mpsc::UnboundedReceiver<(PathBuf, String)>,
) {
    let started = std::time::Instant::now();
    let mut last = 0.0_f64;
    let mut named = false;
    loop {
        let message = match tokio::time::timeout(HEARTBEAT, lines.recv()).await {
            Ok(Some((run_dir, event))) => {
                let run = run_dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let decision = run_dir
                    .parent()
                    .and_then(Path::file_name)
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if named {
                    format!("{run}: {event}")
                } else {
                    named = true;
                    format!("squint://run/{decision}/run.json {run}: {event}")
                }
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
        if peer.notify_progress(param).await.is_err() {
            return;
        }
    }
}

/// Every image path of `job` inside the client's roots (when the client offers roots) and inside
/// the config's `[serve] roots` (when it names any): an image elsewhere is `input`. Each path is
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

/// A fresh run directory, `<action>-<unix ms>-<pid>[-n]`, absolute.
fn new_run_dir(out_dir: &Path, action: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let out_dir = std::path::absolute(out_dir)?;
    let stem = format!("{action}-{}-{}", contract::now_ms(), std::process::id());
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

/// An action's MCP input schema: its parameters, its image slots as images in (`references` a
/// list of them), and the locator and image-block choices.
fn input_schema(action: &table_manners::Action) -> Map<String, Value> {
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
    let mut properties = action.params_schema["properties"]
        .as_object()
        .cloned()
        .unwrap_or_default();
    let mut required: Vec<Value> = action.params_schema["required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for slot in action.images.iter().flatten() {
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
        // A client that offers roots is asked for them on every check: they may have changed.
        // Roots are part of revision 2025-11-25, the only one squint serves; SEP-2577 deprecates
        // them in a later revision, which the contract must answer before squint moves (v0.2).
        #[allow(deprecated)]
        let roots = if (is_check || name == "estimate")
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
                    .filter_map(|r| r.uri.strip_prefix("file://").map(PathBuf::from))
                    .collect(),
            )
        } else {
            None
        };
        let permit = self
            .inner
            .permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let cancel = Arc::new(squint_core::run::Cancel::new());
        let (observer, progress) = match context.meta.get_progress_token().filter(|_| is_check) {
            Some(token) => {
                let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                let observer: squint_core::run::Observer =
                    Arc::new(move |run_dir: &Path, event: &str| {
                        let _ = tx.send((run_dir.to_path_buf(), event.to_string()));
                    });
                let task = tokio::spawn(send_progress(context.peer.clone(), token, rx));
                (Some(observer), Some(task))
            }
            None => (None, None),
        };
        let hooks = Hooks {
            cancel: cancel.clone(),
            observer,
        };
        let task = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            match name.as_str() {
                "describe" => server.describe(),
                "estimate" => server.estimate(args, roots),
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
        Ok(ListResourcesResult::with_all_items(vec![runs]))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        let templates = [
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
