//! The `squint` binary: argument parsing and wiring only. All behaviour lives in the library
//! crates.

mod catalog;
mod describe;
mod eval;
mod playground;
mod report;
mod serve;
mod sets;
mod viewer;

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::provider::Provider;
use squint_core::run::{Cancel, Services};
use squint_core::tool::{Tool, ToolRegistry};

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("SQUINT_GIT_SHA"), ")");

/// Usage error: bad arguments or a configuration the binary cannot act on.
const EXIT_USAGE: i32 = 64;

#[derive(Parser)]
#[command(name = "squint", version = VERSION, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

// Parsed once per process; the size difference between subcommands is irrelevant here.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    /// Judge one image job and print the verdict.
    Check(CheckArgs),
    /// Inspect rules.
    Rules(RulesArgs),
    /// Inspect tools.
    Tools(ToolsArgs),
    /// View a decision directory in the static viewer, rebuild its report, or prune old decisions.
    Report(report::ReportArgs),
    /// Evaluation: the admission gate on a result table (the runner arrives with M3).
    Eval(eval::EvalArgs),
    /// The model catalog: limits, prices, capabilities and sources per model.
    Catalog(catalog::CatalogArgs),
    /// Evaluation sets: check, fetch images, seal and verify a seal.
    Sets(sets::SetsArgs),
    /// Print the table-manners description document: actions, inputs, cost.
    Describe(describe::DescribeArgs),
    /// Print the worst-case cost of one check and whether the cap covers it; nothing is sent.
    Estimate(JobArgs),
    /// Serve the contract's actions over MCP on stdio (protocol 2025-11-25).
    Serve(serve::ServeArgs),
    /// Open Wafflehouse, the shared local workspace, on squint (`wafflehouse --tool squint`).
    Playground(playground::PlaygroundArgs),
}

#[derive(clap::Args)]
struct ToolsArgs {
    #[command(subcommand)]
    command: ToolsCommand,
}

#[derive(Subcommand)]
enum ToolsCommand {
    /// Print the manifests of the enabled tools as the model will see them.
    List(ToolsListArgs),
    /// Install a tool pack's provisioning bundle: verify digests, unpack, `uv sync --frozen`,
    /// then the pack's `fetch` of its model artifacts.
    Setup(ToolsSetupArgs),
}

#[derive(clap::Args)]
struct ToolsSetupArgs {
    /// A bundle directory (`bundle.toml`, the wheel, the provisioning project and its lock).
    #[arg(long)]
    bundle: PathBuf,
    /// Where to install; defaults to `$XDG_DATA_HOME` or `~/.local/share`.
    #[arg(long)]
    data_home: Option<PathBuf>,
    /// After the sync, have the pack fetch its model artifacts from this directory instead of
    /// the release named in its record (the export's output, before publication).
    #[arg(long, conflicts_with = "no_fetch")]
    models_from: Option<PathBuf>,
    /// Do not run the pack's `fetch` after the sync; tools that need an artifact report it
    /// missing until `squint-tools fetch` has run.
    #[arg(long)]
    no_fetch: bool,
}

#[derive(clap::Args)]
struct ToolsListArgs {
    /// Path to the config file; `[tools] enabled` names the tools.
    #[arg(long)]
    config: PathBuf,
    /// Print JSON instead of the text form.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct RulesArgs {
    #[command(subcommand)]
    command: RulesCommand,
}

#[derive(Subcommand)]
enum RulesCommand {
    /// Print the merged obligations for a config (and optionally a job) with provenance.
    Show(RulesShowArgs),
}

#[derive(clap::Args)]
struct RulesShowArgs {
    /// Path to the config file; `[rules]` names the base, profile and extra files.
    #[arg(long)]
    config: PathBuf,
    /// Optional job file, so scope, evidence and required roles are checked against it.
    #[arg(long)]
    job: Option<PathBuf>,
    /// Print JSON instead of the table.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct CheckArgs {
    #[command(flatten)]
    input: JobArgs,
    /// Output directory for run directories; overrides `[output] dir`.
    #[arg(long)]
    out: Option<PathBuf>,
    /// Resolve the rules and print the Obligations section, the instances and the checklist
    /// without calling any provider.
    #[arg(long)]
    dry_run: bool,
    /// Replay a recorded conversation (a directory of exchanges) instead of calling the provider;
    /// no credential is needed and nothing leaves the machine.
    #[arg(long, value_name = "DIR", conflicts_with = "record")]
    replay: Option<PathBuf>,
    /// Record every exchange with the provider into this directory, secrets stripped.
    #[arg(long, value_name = "DIR")]
    record: Option<PathBuf>,
}

/// What `check` and `estimate` share: the config and the job.
#[derive(clap::Args)]
struct JobArgs {
    /// Path to the config file.
    #[arg(long)]
    config: PathBuf,
    /// Override one config value: `--set budget.tool_turns=2` (repeatable).
    #[arg(long = "set", value_name = "PATH=VALUE")]
    overrides: Vec<String>,
    /// Path to a job file. Relative image paths in it resolve against the current directory.
    #[arg(long, conflicts_with_all = ["candidate", "source", "reference", "brief", "use_case"])]
    job: Option<PathBuf>,
    /// Shorthand job: the candidate image.
    #[arg(long, requires = "use_case")]
    candidate: Option<PathBuf>,
    /// Shorthand job: the source image the candidate was made from.
    #[arg(long, requires = "candidate")]
    source: Option<PathBuf>,
    /// Shorthand job: a reference image as `label=path` (repeatable); its id becomes `ref:<label>`.
    #[arg(long = "ref", value_name = "LABEL=PATH", requires = "candidate")]
    reference: Vec<String>,
    /// Shorthand job: the free-text brief.
    #[arg(long, requires = "candidate")]
    brief: Option<String>,
    /// Shorthand job: the use case, for example `source-fidelity`.
    #[arg(long, requires = "candidate")]
    use_case: Option<String>,
    /// Lower the configured per-call spend cap for this check; it never raises it.
    #[arg(long, value_name = "USD")]
    max_spend_usd: Option<f64>,
    /// Scripted fake provider responses, for tests and debugging only.
    #[cfg(feature = "testkit")]
    #[arg(long)]
    provider_script: Option<PathBuf>,
    /// Scripted envelopes for the fake `fake_regions` tool, for tests and debugging only.
    #[cfg(feature = "testkit")]
    #[arg(long)]
    tool_script: Option<PathBuf>,
}

fn main() {
    // Argument errors exit 64 like every other usage error (table-manners 6); help and the
    // version print to stdout and exit 0.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            let code = if e.use_stderr() { EXIT_USAGE } else { 0 };
            let _ = e.print();
            std::process::exit(code);
        }
    };
    let code = match cli.command {
        None => {
            eprintln!("squint {VERSION}: no command given; try --help");
            EXIT_USAGE
        }
        Some(Command::Check(args)) => check(&args),
        Some(Command::Rules(RulesArgs {
            command: RulesCommand::Show(args),
        })) => rules_show(&args),
        Some(Command::Tools(ToolsArgs {
            command: ToolsCommand::List(args),
        })) => tools_list(&args),
        Some(Command::Tools(ToolsArgs {
            command: ToolsCommand::Setup(args),
        })) => tools_setup(&args),
        Some(Command::Report(args)) => report::run(&args),
        Some(Command::Eval(args)) => eval::run(&args),
        Some(Command::Catalog(args)) => catalog::run(&args),
        Some(Command::Sets(args)) => sets::run(&args),
        Some(Command::Describe(args)) => describe::run(&args),
        Some(Command::Estimate(args)) => estimate(&args),
        Some(Command::Serve(args)) => serve::run(&args),
        Some(Command::Playground(args)) => playground::run(&args),
    };
    std::process::exit(code);
}

fn check(args: &CheckArgs) -> i32 {
    let mut config = match load_config(&args.input) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    // Resolve-time checks against the catalog (profile, model, controls, image size, allowances)
    // and the consent binding. The scripted pair passes in testkit builds, and in a dry run,
    // where nothing is sent.
    if let Err(e) = squint_providers::catalog::Catalog::shipped().and_then(|c| {
        c.check_config(&config, cfg!(feature = "testkit") || args.dry_run)?;
        match c.profile(&config.provider.profile) {
            Some(profile) => {
                squint_providers::privacy::consent(&config, &config.provider.profile, profile)
                    .map(|_| ())
            }
            None => Ok(()),
        }
    }) {
        eprintln!("squint: {e}");
        return EXIT_USAGE;
    }
    let job = match load_job(&args.input) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let tools = match build_tools(&args.input, &config) {
        Ok(t) => t,
        Err(message) => {
            eprintln!("squint: {message}");
            return EXIT_USAGE;
        }
    };
    // Every enabled tool and `submit_verdict` must have a wire form for the selected model, and
    // the set must fit the provider's limits, before any billable request.
    if let Err(e) = check_wire_schemas(&config, &tools) {
        eprintln!("squint: {e}");
        return EXIT_USAGE;
    }
    if args.dry_run {
        return match squint_core::run::resolve_rules(&config, &job, Path::new("."), &tools) {
            Ok(p) => {
                println!("{}", p.system_prompt);
                println!("Instances:");
                for i in &p.resolved.instances {
                    println!(
                        "  {} kind={:?} matters={} hidden_ok={} evidence={:?} kinds={:?} [{}]",
                        i.id(),
                        i.kind,
                        i.matters,
                        i.hidden_ok,
                        i.evidence,
                        i.evidence_kinds,
                        i.provenance
                    );
                }
                println!("Checklist:");
                for c in &p.checklist.items {
                    println!("  {} <- {:?}", c.id, c.instance_ids);
                }
                for w in &p.summary.warnings {
                    eprintln!("warning: {w}");
                }
                0
            }
            Err(e) => {
                eprintln!("squint: {e}");
                EXIT_USAGE
            }
        };
    }
    let provider: Box<dyn Provider> = match select_provider(args, &mut config) {
        Ok(p) => p,
        Err(message) => {
            eprintln!("squint: {message}");
            return EXIT_USAGE;
        }
    };
    let out_dir = args
        .out
        .clone()
        .unwrap_or_else(|| config.output.dir.clone());
    let job_base = Path::new(".");

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("squint: cannot start the runtime: {e}");
            return EXIT_USAGE;
        }
    };
    let cancel = std::sync::Arc::new(Cancel::new());
    // The exit code of the signal that cancelled the run, 0 until one arrives.
    let signalled = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
    // The handlers are registered here, before the run starts, so a signal that lands during the
    // run's first synchronous work is caught rather than taking the process down.
    let signals = {
        let _guard = runtime.enter();
        install_signal_handlers()
    };
    runtime.spawn(cancel_on_signal(
        signals,
        std::sync::Arc::clone(&cancel),
        std::sync::Arc::clone(&signalled),
    ));
    let services = Services::new(provider.as_ref(), &tools).with_cancel(&cancel);
    let outcome = runtime.block_on(squint_core::run::check(
        &config, &job, job_base, &services, &out_dir, VERSION,
    ));
    match outcome {
        Ok(o) => {
            match (&o.decision.verdict, &o.error) {
                (Some(v), _) => println!(
                    "{}",
                    serde_json::to_string(v)
                        .unwrap_or_default()
                        .trim_matches('"')
                ),
                (None, Some(e)) => eprintln!("error: {}", redacted(e, &config)),
                (None, None) => eprintln!("error: run ended without a verdict or an error"),
            }
            if o.cache.is_hit() {
                eprintln!(
                    "cache: hit (source: {})",
                    o.cache
                        .source
                        .as_deref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                );
            }
            eprintln!("decision: {}", o.dir.display());
            match signalled.load(std::sync::atomic::Ordering::SeqCst) {
                0 => o.exit_code,
                code if o.decision.verdict.is_none() => code,
                _ => o.exit_code,
            }
        }
        Err(e) => {
            eprintln!("squint: {}", redacted(&e.to_string(), &config));
            // A signal that landed before the decision was allocated still exits as signalled.
            match signalled.load(std::sync::atomic::Ordering::SeqCst) {
                0 => squint_core::run::EXIT_ERROR,
                code => code,
            }
        }
    }
}

/// The registered signal streams (unix) or nothing (elsewhere, where `ctrl_c` registers on await).
#[cfg(unix)]
type Signals = Option<(tokio::signal::unix::Signal, tokio::signal::unix::Signal)>;
#[cfg(not(unix))]
type Signals = ();

/// Registers SIGINT and SIGTERM with the runtime; must run inside a runtime context.
#[cfg(unix)]
fn install_signal_handlers() -> Signals {
    use tokio::signal::unix::{SignalKind, signal};
    match (
        signal(SignalKind::interrupt()),
        signal(SignalKind::terminate()),
    ) {
        (Ok(int), Ok(term)) => Some((int, term)),
        _ => None,
    }
}

#[cfg(not(unix))]
fn install_signal_handlers() -> Signals {}

/// The exit code of a run cancelled by SIGINT (table-manners 6).
const EXIT_SIGINT: i32 = 130;
/// The exit code of a run cancelled by SIGTERM.
const EXIT_SIGTERM: i32 = 143;

/// SIGINT or SIGTERM cancels the run; it appends `cancelled`, and the process exits 130 or 143.
async fn cancel_on_signal(
    signals: Signals,
    cancel: std::sync::Arc<Cancel>,
    signalled: std::sync::Arc<std::sync::atomic::AtomicI32>,
) {
    #[cfg(unix)]
    let code = match signals {
        Some((mut int, mut term)) => {
            tokio::select! {
                _ = int.recv() => EXIT_SIGINT,
                _ = term.recv() => EXIT_SIGTERM,
            }
        }
        None => {
            let _ = tokio::signal::ctrl_c().await;
            EXIT_SIGINT
        }
    };
    #[cfg(not(unix))]
    let code = {
        let _ = signals;
        let _ = tokio::signal::ctrl_c().await;
        EXIT_SIGINT
    };
    signalled.store(code, std::sync::atomic::Ordering::SeqCst);
    cancel.cancel();
}

/// Error text never carries an expanded secret.
fn redacted(text: &str, config: &Config) -> String {
    let mut out = text.to_string();
    for s in &config.secrets {
        if !s.is_empty() {
            out = out.replace(s, "[redacted]");
        }
    }
    out
}

/// `squint estimate`: the envelope `check` would reserve, priced from the catalog (or, in a
/// testkit build, the scripted provider); nothing is sent and no credential is read.
fn estimate(args: &JobArgs) -> i32 {
    let usage = |e: String| {
        eprintln!("squint: {e}");
        EXIT_USAGE
    };
    let config = match load_config(args) {
        Ok(c) => c,
        Err(e) => return usage(e),
    };
    let job = match load_job(args) {
        Ok(j) => j,
        Err(e) => return usage(e),
    };
    let tools = match build_tools(args, &config) {
        Ok(t) => t,
        Err(e) => return usage(e),
    };
    let catalog = match squint_providers::catalog::Catalog::shipped() {
        Ok(c) => c,
        Err(e) => return usage(e.to_string()),
    };
    #[cfg(feature = "testkit")]
    let scripted = match args
        .provider_script
        .as_deref()
        .map(squint_testkit::ScriptedProvider::from_path)
        .transpose()
    {
        Ok(p) => p,
        Err(e) => return usage(e.to_string()),
    };
    #[cfg(feature = "testkit")]
    let scripted_pricing = scripted.as_ref().and_then(|p| p.pricing());
    #[cfg(not(feature = "testkit"))]
    let scripted_pricing: Option<&dyn squint_core::ledger::Pricing> = None;
    let pricing: &dyn squint_core::ledger::Pricing = match scripted_pricing {
        Some(p) => p,
        None => match catalog.model(&config.provider.model) {
            Some(model) => model,
            None => {
                return usage(format!(
                    "model {:?} is not in the catalog",
                    config.provider.model
                ));
            }
        },
    };
    match squint_core::run::envelope(&config, &job, Path::new("."), &tools, pricing) {
        Ok(envelope) => {
            let cap = config.budget.spend_cap_usd;
            let out = serde_json::json!({
                "use_case": job.use_case,
                "model": config.provider.model,
                "envelope_usd": envelope.usd,
                "cap_usd": cap,
                "fits": envelope.usd <= cap,
                "decision_runs": envelope.decision_runs,
                "requests": envelope.requests.len(),
                "attempts": envelope.attempts,
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
            0
        }
        Err(e) => usage(e.to_string()),
    }
}

/// The config with `--set` overrides and a `--max-spend-usd` that lowers the cap.
fn load_config(args: &JobArgs) -> Result<Config, String> {
    let mut config =
        Config::load_with_overrides(&args.config, &args.overrides).map_err(|e| e.to_string())?;
    if let Some(max) = args.max_spend_usd {
        config.lower_spend_cap(max).map_err(|e| e.to_string())?;
    }
    Ok(config)
}

fn load_job(args: &JobArgs) -> Result<Job, String> {
    match (&args.job, &args.candidate) {
        (Some(path), None) => Job::load(path).map_err(|e| e.to_string()),
        (None, Some(candidate)) => {
            let use_case = args.use_case.as_deref().unwrap_or("source-fidelity");
            let mut refs = Vec::new();
            for r in &args.reference {
                let (label, path) = r
                    .split_once('=')
                    .ok_or_else(|| format!("--ref {r:?} must be label=path"))?;
                refs.push((label.to_string(), PathBuf::from(path)));
            }
            Job::from_shorthand(
                use_case,
                candidate.clone(),
                args.source.clone(),
                &refs,
                args.brief.clone(),
            )
            .map_err(|e| e.to_string())
        }
        (None, None) => Err("pass --job <file> or the shorthand --candidate … --use-case …".into()),
        (Some(_), Some(_)) => Err("--job and --candidate are exclusive".into()),
    }
}

/// The provider for this run: the scripted fake (testkit builds, `--provider-script`), a replay of
/// a recorded conversation, or the live adapter for the configured profile and model over HTTPS,
/// optionally recording every exchange. The credential joins the redaction list before any run.
/// Where a provider's conversation comes from or goes to, beyond the live endpoint.
#[derive(Default)]
struct ProviderOptions<'a> {
    replay: Option<&'a Path>,
    record: Option<&'a Path>,
    #[cfg(feature = "testkit")]
    provider_script: Option<&'a Path>,
}

fn select_provider(args: &CheckArgs, config: &mut Config) -> Result<Box<dyn Provider>, String> {
    let opts = ProviderOptions {
        replay: args.replay.as_deref(),
        record: args.record.as_deref(),
        #[cfg(feature = "testkit")]
        provider_script: args.input.provider_script.as_deref(),
    };
    provider_for(config, &opts)
}

/// The provider for a config: the catalog's adapter over a live client, a recording or, in a
/// testkit build, a script. A live credential is added to the config's secrets.
/// What a check under `config` runs on, for the description's cost (squint issue #2): the
/// scripted provider (`--provider-script`, or the fake profile, which runs only with one) can
/// never charge; any catalog profile may, and names its credential variable.
fn executor(config: &Config, scripted: bool) -> squint_core::contract::Executor {
    use squint_core::contract::Executor;
    use squint_providers::catalog::{Catalog, FAKE_PROFILE};
    if scripted || config.provider.profile == FAKE_PROFILE {
        return Executor::Free;
    }
    // The variable's name comes from the catalog's profile; its value is never read here.
    let needs_env = Catalog::shipped()
        .ok()
        .and_then(|c| {
            c.profile(&config.provider.profile)
                .map(|p| p.auth.env.clone())
        })
        .into_iter()
        .collect();
    Executor::Paid { needs_env }
}

fn provider_for(
    config: &mut Config,
    opts: &ProviderOptions<'_>,
) -> Result<Box<dyn Provider>, String> {
    use std::sync::Arc;

    use squint_providers::anthropic::AnthropicProvider;
    use squint_providers::catalog::{Catalog, FAKE_PROFILE};
    use squint_providers::http::{Client, Transport};
    use squint_providers::privacy::credential;
    use squint_providers::replay::{Player, Recorder};
    #[cfg(feature = "testkit")]
    if let Some(path) = opts.provider_script {
        return squint_testkit::ScriptedProvider::from_path(path)
            .map(|p| Box::new(p) as Box<dyn Provider>)
            .map_err(|e| e.to_string());
    }
    if config.provider.profile == FAKE_PROFILE {
        return Err("the fake profile runs only with --provider-script in a testkit build".into());
    }
    let catalog = Catalog::shipped().map_err(|e| e.to_string())?;
    let profile = catalog.profile(&config.provider.profile).ok_or_else(|| {
        format!(
            "profile {:?} is not in the catalog",
            config.provider.profile
        )
    })?;
    let model = catalog
        .model(&config.provider.model)
        .ok_or_else(|| format!("model {:?} is not in the catalog", config.provider.model))?;
    // Which request headers and body pointers a recording treats as volatile is the adapter's.
    let volatile = match model.adapter.as_str() {
        "anthropic-messages" => AnthropicProvider::volatile(),
        "gemini-generate-content" => squint_providers::gemini::GeminiProvider::volatile(),
        "openai-responses" => squint_providers::openai::OpenAiProvider::volatile(),
        other => return Err(format!("adapter {other} is not implemented")),
    };
    let (cred, transport): (_, Arc<dyn Transport>) = match opts.replay {
        Some(dir) => {
            let cred =
                credential(profile, &|_| Some("replay".into())).map_err(|e| e.to_string())?;
            let player = Player::load(dir, volatile).map_err(|e| e.to_string())?;
            (cred, Arc::new(player))
        }
        None => {
            let cred =
                credential(profile, &|name| std::env::var(name).ok()).map_err(|e| e.to_string())?;
            config.secrets.push(cred.expose().to_string());
            let client = Client::new(std::time::Duration::from_secs(
                config.budget.request_timeout_s,
            ))
            .map_err(|e| e.to_string())?;
            let base: Arc<dyn Transport> = Arc::new(client);
            match opts.record {
                Some(dir) => {
                    let recorder =
                        Recorder::new(base, dir, vec![cred.expose().to_string()], volatile)
                            .map_err(|e| e.to_string())?;
                    (cred, Arc::new(recorder))
                }
                None => (cred, base),
            }
        }
    };
    let id = &config.provider.profile;
    match model.adapter.as_str() {
        "anthropic-messages" => AnthropicProvider::new(config, id, profile, model, cred, transport)
            .map(|p| Box::new(p) as Box<dyn Provider>)
            .map_err(|e| e.to_string()),
        "gemini-generate-content" => squint_providers::gemini::GeminiProvider::new(
            config, id, profile, model, cred, transport,
        )
        .map(|p| Box::new(p) as Box<dyn Provider>)
        .map_err(|e| e.to_string()),
        "openai-responses" => squint_providers::openai::OpenAiProvider::new(
            config, id, profile, model, cred, transport,
        )
        .map(|p| Box::new(p) as Box<dyn Provider>)
        .map_err(|e| e.to_string()),
        other => Err(format!("adapter {other} is not implemented")),
    }
}

fn rules_show(args: &RulesShowArgs) -> i32 {
    use squint_core::obligations::{MergeInput, RulesFile, merge};
    let config = match Config::load(&args.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let Some(rules) = &config.rules else {
        eprintln!("squint: the config has no [rules] section");
        return EXIT_USAGE;
    };
    let job = match &args.job {
        Some(path) => match Job::load(path) {
            Ok(j) => Some(j),
            Err(e) => {
                eprintln!("squint: {e}");
                return EXIT_USAGE;
            }
        },
        None => None,
    };
    let mut extra = Vec::new();
    let mut names = Vec::new();
    for path in &rules.extra {
        match RulesFile::load(path) {
            Ok(f) => {
                extra.push(f);
                names.push(path.display().to_string());
            }
            Err(e) => {
                eprintln!("squint: {e}");
                return EXIT_USAGE;
            }
        }
    }
    let merged = merge(&MergeInput {
        base: &rules.base,
        profile: rules.profile.as_deref(),
        extra: &extra,
        extra_names: &names,
        job: job.as_ref(),
        enabled_tools: &config.tools.enabled,
    });
    match merged {
        Ok(m) => {
            if args.json {
                println!("{}", serde_json::to_string_pretty(&m).unwrap_or_default());
            } else {
                println!(
                    "use case: {}  base: {}  profile: {}",
                    m.use_case,
                    m.base,
                    m.profile.as_deref().unwrap_or("-")
                );
                for o in &m.obligations {
                    let ob = &o.obligation;
                    println!(
                        "{:<5} {:<10} matters={:<5} hidden_ok={:<5} [{}] {}",
                        ob.id,
                        format!("{:?}", ob.kind).to_lowercase(),
                        ob.matters(),
                        ob.hidden_ok,
                        o.provenance,
                        ob.text
                    );
                }
                for w in &m.warnings {
                    eprintln!("warning: {w}");
                }
            }
            0
        }
        Err(e) => {
            eprintln!("squint: {e}");
            EXIT_USAGE
        }
    }
}

/// The wire transformation of every tool definition for the configured model (SPEC "Wire
/// schemas"). The scripted pair has no adapter and nothing to check.
fn check_wire_schemas(
    config: &Config,
    tools: &squint_core::tool::ToolRegistry,
) -> squint_core::Result<()> {
    use squint_providers::catalog::{Catalog, FAKE_MODEL};
    use squint_providers::wire::{Adapter, check_tool_set};
    if config.provider.model == FAKE_MODEL {
        return Ok(());
    }
    let catalog = Catalog::shipped()?;
    let Some(model) = catalog.model(&config.provider.model) else {
        return Ok(());
    };
    let Some(adapter) = Adapter::parse(&model.adapter) else {
        return Err(squint_core::Error::Config(format!(
            "catalog: model {:?} names an adapter this build lacks: {}",
            config.provider.model, model.adapter
        )));
    };
    let mut definitions: Vec<squint_core::provider::ToolDefinition> =
        tools.manifests().map(|m| m.tool_definition()).collect();
    definitions.push(squint_core::submission::submit_verdict_tool());
    check_tool_set(adapter, &model.capabilities, &definitions).map(|_| ())
}

/// The tools this build knows. Built-ins arrive in M4; the fake tool exists behind `testkit`.
/// The manifests this build knows without a registry: the built-ins and, in a testkit build, the
/// scripted tool.
fn known_manifests() -> Vec<squint_core::tool::Manifest> {
    let builtins = squint_tools::builtin::manifests();
    #[cfg(feature = "testkit")]
    {
        let mut known = builtins;
        known.push(
            squint_core::tool::Manifest::from_toml(squint_testkit::tool::FAKE_REGIONS_MANIFEST)
                .expect("fake_regions manifest is valid"),
        );
        known
    }
    #[cfg(not(feature = "testkit"))]
    builtins
}

fn tools_list(args: &ToolsListArgs) -> i32 {
    let config = match Config::load(&args.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let mut known = known_manifests();
    let packs = match connect_packs(&config, &SharedPacks::default()) {
        Ok(p) => p,
        Err(message) => {
            eprintln!("squint: {message}");
            return EXIT_USAGE;
        }
    };
    for (pack, _) in &packs {
        known.extend(pack.manifests().iter().cloned());
    }
    let mut listed = Vec::new();
    for name in &config.tools.enabled {
        match known.iter().find(|m| &m.name == name) {
            Some(m) => listed.push(m.clone()),
            None => {
                eprintln!("squint: tool {name:?} is enabled but not known to this build");
                return EXIT_USAGE;
            }
        }
    }
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&listed).unwrap_or_default()
        );
        return 0;
    }
    for m in &listed {
        println!(
            "{} v{} ({:?}, ~{}s)",
            m.name, m.version, m.cost.tier, m.cost.seconds
        );
        println!("  purpose: {}", m.purpose);
        println!("  when to use: {}", m.when_to_use);
        println!("  when not to use: {}", m.when_not_to_use);
        println!("  output: {}", m.output_guidance);
        if let Some(k) = &m.evidence_kind {
            println!("  evidence kind: {k}");
        }
        if !m.requires.is_empty() {
            println!("  requires: {}", m.requires.join(", "));
        }
        if !m.projections.is_empty() {
            println!("  projections: {}", m.projections.join(", "));
        }
        println!(
            "  model schema: {}",
            serde_json::to_string(&m.model_schema).unwrap_or_default()
        );
    }
    0
}

/// The tools available to this build, then narrowed to the config's enabled list.
fn build_tools(args: &JobArgs, config: &Config) -> Result<ToolRegistry, String> {
    #[cfg(feature = "testkit")]
    let script = args.tool_script.as_deref();
    #[cfg(not(feature = "testkit"))]
    let script = {
        let _ = args;
        None
    };
    tools_for_config(config, script, &SharedPacks::default())
}

/// The tool registry for a config: the configured packs, then the built-in (and in a testkit
/// build the scripted) tools, enabled and ordered as the config says; carrying the lock digest
/// of the installed pack each enabled pack tool came from (a pack run by `command` has none).
fn tools_for_config(
    config: &Config,
    tool_script: Option<&Path>,
    shared: &SharedPacks,
) -> Result<ToolRegistry, String> {
    // Packs first; a built-in or scripted tool of the same name yields to the pack's.
    let mut available: Vec<Box<dyn Tool>> = Vec::new();
    let mut digests = std::collections::BTreeMap::new();
    for (pack, lock_digest) in connect_packs(config, shared)? {
        for tool in pack.tools() {
            if let Some(d) = &lock_digest {
                digests.insert(tool.manifest().name.clone(), d.clone());
            }
            available.push(tool);
        }
    }
    for tool in available_tools(tool_script)? {
        if !available
            .iter()
            .any(|t| t.manifest().name == tool.manifest().name)
        {
            available.push(tool);
        }
    }
    let registry = ToolRegistry::new(available, &config.tools.enabled, &config.tools.order)
        .map_err(|e| e.to_string())?;
    digests.retain(|name, _| config.tools.enabled.contains(name));
    Ok(registry.with_lock_digests(digests))
}

fn tools_setup(args: &ToolsSetupArgs) -> i32 {
    use squint_tools::provision::{Bundle, Fetch, data_home};
    let home = args.data_home.clone().unwrap_or_else(data_home);
    let bundle = match Bundle::load(&args.bundle) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let fetch = match (&args.models_from, args.no_fetch) {
        (Some(dir), _) => Fetch::From {
            dir: dir.clone(),
            models: None,
        },
        (None, true) => Fetch::Skip,
        (None, false) => Fetch::Release,
    };
    match bundle.install(&home, &fetch) {
        Ok(installed) => {
            println!(
                "installed {} {} into {}",
                installed.header.package,
                installed.header.version,
                installed.dir.display()
            );
            println!("executable: {}", installed.executable.display());
            println!("lock digest: {}", installed.lock_digest);
            0
        }
        Err(e) => {
            eprintln!("squint: {e}");
            EXIT_USAGE
        }
    }
}

/// Connects every pack the config names: by `command`, or by `package` and `version` from an
/// installed bundle.
/// A connected pack and, when it came from an installed bundle, that bundle's lock digest.
pub(crate) type ConnectedPack = (std::sync::Arc<squint_tools::mcp::Pack>, Option<String>);

/// Packs configured `shared = true`, connected once and reused across the decisions of an
/// evaluation under their leases (SPEC "Lifetime and leases"); keyed by pack name.
#[derive(Default)]
pub(crate) struct SharedPacks(std::sync::Mutex<std::collections::BTreeMap<String, ConnectedPack>>);

fn connect_packs(config: &Config, shared: &SharedPacks) -> Result<Vec<ConnectedPack>, String> {
    use squint_tools::mcp::{Pack, Spawn, Timeouts};
    use squint_tools::provision::{Installed, data_home};
    use std::time::Duration;
    let mut packs = Vec::new();
    for (name, pack) in &config.tools.packs {
        if pack.shared
            && let Ok(held) = shared.0.lock()
            && let Some(connected) = held.get(name)
        {
            packs.push((std::sync::Arc::clone(&connected.0), connected.1.clone()));
            continue;
        }
        let (spawn, lock_digest) = match (&pack.command, &pack.package, &pack.version) {
            (Some(command), _, _) => (
                Spawn {
                    command: command.clone(),
                    args: pack.args.clone(),
                    declared_env: pack.env.clone(),
                    cwd: None,
                    extra_env: Vec::new(),
                },
                None,
            ),
            (None, Some(package), Some(version)) => {
                let installed =
                    Installed::locate(&data_home(), package, version).map_err(|e| e.to_string())?;
                let digest = installed.lock_digest.clone();
                (installed.spawn(pack.env.clone()), Some(digest))
            }
            _ => continue,
        };
        let timeouts = Timeouts {
            startup: Duration::from_secs(pack.startup_timeout_s),
            call: Duration::from_secs(config.budget.tool_timeout_s),
            shutdown_grace: Duration::from_secs(5),
        };
        let connected = (
            Pack::connect(name, spawn, timeouts, std::env::vars()).map_err(|e| e.to_string())?,
            lock_digest,
        );
        if pack.shared
            && let Ok(mut held) = shared.0.lock()
        {
            held.insert(
                name.clone(),
                (std::sync::Arc::clone(&connected.0), connected.1.clone()),
            );
        }
        packs.push(connected);
    }
    Ok(packs)
}

#[cfg(feature = "testkit")]
fn available_tools(tool_script: Option<&Path>) -> Result<Vec<Box<dyn Tool>>, String> {
    let mut out: Vec<Box<dyn Tool>> = squint_tools::builtin::all();
    match tool_script {
        Some(path) => out.push(Box::new(
            squint_testkit::ScriptedTool::fake_regions_from_path(path)
                .map_err(|e| e.to_string())?,
        )),
        None => out.push(Box::new(squint_testkit::ScriptedTool::fake_regions(
            Vec::new(),
        ))),
    }
    Ok(out)
}

#[cfg(not(feature = "testkit"))]
fn available_tools(_tool_script: Option<&Path>) -> Result<Vec<Box<dyn Tool>>, String> {
    Ok(squint_tools::builtin::all())
}
