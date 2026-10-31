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
    Implementation, InitializeResult, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
    ServerCapabilities, Tool,
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
    let description = contract::description(&config, VERSION, &needs_env);
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
    fn estimate(&self, args: Map<String, Value>) -> Result<CallToolResult, ErrorData> {
        let use_case = args
            .get("use_case")
            .and_then(Value::as_str)
            .ok_or_else(|| ErrorData::invalid_params("use_case: required", None))?
            .to_string();
        let job = match contract::job_from_call(&use_case, &args) {
            Ok(job) => job,
            Err(squint_core::Error::Job(m)) => return Err(ErrorData::invalid_params(m, None)),
            Err(e) => return self.finished("estimate", &args, Some(&e), Map::new()),
        };
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
        match squint_core::run::envelope(&config, &job, Path::new("/"), &self.inner.tools, pricing)
        {
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
    ) -> Result<CallToolResult, ErrorData> {
        let use_case = action.replace('_', "-");
        let job = match contract::job_from_call(&use_case, &args) {
            Ok(job) => job,
            Err(squint_core::Error::Job(m)) => return Err(ErrorData::invalid_params(m, None)),
            Err(e) => return self.finished(action, &args, Some(&e), Map::new()),
        };
        if let Err(e) = inside_roots(&job, roots.as_deref(), &self.inner.config.serve.roots) {
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
        let services = squint_core::run::Services::new(provider.as_ref(), &self.inner.tools)
            .with_shared_ledger(Some(&self.inner.session));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let outcome = runtime.block_on(squint_core::run::check(
            &config,
            &job,
            Path::new("/"),
            &services,
            &self.inner.out_dir,
            VERSION,
        ));
        match outcome {
            Ok(o) => {
                let text = std::fs::read_to_string(o.dir.join(contract::RUN_FILE))
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                let mut result: table_manners::RunResult = serde_json::from_str(&text)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                // The decision is the action's: `run.json` names it so.
                result.action = action.to_string();
                contract::write_run_result(&o.dir, &result)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                Ok(tool_result(&result))
            }
            Err(e) => self.finished(action, &args, Some(&e), Map::new()),
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

/// Every image path of `job` inside the client's roots (when the client offers roots) and inside
/// the config's `[serve] roots` (when it names any): an image elsewhere is `input`.
fn inside_roots(
    job: &squint_core::job::Job,
    client: Option<&[PathBuf]>,
    config: &[PathBuf],
) -> squint_core::Result<()> {
    let within = |path: &Path, roots: &[PathBuf]| {
        roots
            .iter()
            .any(|root| root.canonicalize().is_ok_and(|r| path.starts_with(r)))
    };
    for image in &job.images {
        let squint_core::job::ImageSource::Path { path, .. } = &image.source else {
            continue;
        };
        let real = path.canonicalize().map_err(|e| {
            squint_core::Error::Input(format!("{}: {}: {e}", image.id, path.display()))
        })?;
        if client.is_some_and(|roots| !within(&real, roots)) {
            return Err(squint_core::Error::Input(format!(
                "{}: {} is outside the client's roots",
                image.id,
                path.display()
            )));
        }
        if !config.is_empty() && !within(&real, config) {
            return Err(squint_core::Error::Input(format!(
                "{}: {} is outside [serve] roots",
                image.id,
                path.display()
            )));
        }
    }
    Ok(())
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
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
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
        let roots = if is_check
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
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            match name.as_str() {
                "describe" => server.describe(),
                "estimate" => server.estimate(args),
                action if is_check => server.check(action, args, roots),
                other => Err(ErrorData::invalid_params(
                    format!("no tool {other:?}; call describe for the actions"),
                    None,
                )),
            }
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("the call panicked: {e}"), None))??;
        Ok(result.into())
    }
}
