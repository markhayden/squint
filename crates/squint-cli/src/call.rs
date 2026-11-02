//! `squint call <action> --arguments <file>`: the contract's generic command (table-manners 4.8,
//! task 10.4). It runs any action from exactly the arguments an MCP call to it would send, through
//! the code `serve` uses (`contract::job_from_call`, the path rule's allowed directories, the
//! refused run), so the command and the server cannot drift. So an `invocation`'s `arguments`,
//! with a locator added to each image, are the file for `squint call <invocation.action>`.
//!
//! `squint call --bundle <path>` (4.8, 4.9, task 10.5b) replays an invocation bundle through the
//! same code: the bundle path under the path rule, the bundle read and checked whole
//! (`squint_core::bundle`), its input members copied into a directory of squint's own, and each
//! step called with its `invocation.arguments` plus a `path` beside each image's `expect`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use serde_json::{Map, Value};
use squint_core::bundle::Bundle;
use squint_core::config::Config;
use squint_core::contract;
use squint_core::readiness::{self, Answers, Needs};
use squint_core::run::{Cancel, Services};

use crate::{EXIT_USAGE, VERSION};

#[derive(clap::Args)]
pub struct CallArgs {
    /// The action to run, as `squint describe --json` names it (`source_fidelity`, …).
    #[arg(required_unless_present = "bundle", conflicts_with = "bundle")]
    pub action: Option<String>,
    /// A JSON file holding exactly the arguments an MCP call to the action would send.
    #[arg(
        long,
        value_name = "FILE",
        required_unless_present = "bundle",
        conflicts_with = "bundle"
    )]
    pub arguments: Option<PathBuf>,
    /// Replay an invocation bundle instead (table-manners 4.9): the archive or its unpacked
    /// directory, inside an `--allow-dir`. Each step runs as a call of its own; several print one
    /// result per line, stop at the first that does not complete, and otherwise exit with the most
    /// serious step's code.
    #[arg(long, value_name = "PATH")]
    pub bundle: Option<PathBuf>,
    /// Path to the config file; without it, squint looks for ./squint.toml, then
    /// $XDG_CONFIG_HOME/squint/squint.toml.
    #[arg(long)]
    pub config: Option<PathBuf>,
    /// A directory image paths in the arguments, or the bundle, may be read from: absolute and
    /// existing (repeatable). `[serve] allow_dirs` may narrow them, never widen; with none, every
    /// path is refused and images come inline (table-manners 5.2, as for `serve`).
    #[arg(long = "allow-dir", value_name = "ABS_DIR")]
    pub allow_dirs: Vec<PathBuf>,
    /// Output directory for run directories; overrides `[output] dir`.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Override one config value: `--set budget.tool_turns=2` (repeatable).
    #[arg(long = "set", value_name = "PATH=VALUE")]
    pub overrides: Vec<String>,
    /// Scripted fake provider responses, for tests and debugging only.
    #[cfg(feature = "testkit")]
    #[arg(long)]
    pub provider_script: Option<PathBuf>,
}

/// `squint call`: one action from an arguments file, or the steps of a bundle.
pub fn run(args: &CallArgs) -> i32 {
    match (&args.bundle, &args.action, &args.arguments) {
        (Some(bundle), _, _) => replay(args, bundle),
        (None, Some(action), Some(file)) => match read_arguments(file) {
            Ok(call) => one(args, action, call, Dirs::Flags, Print::Pretty).code,
            Err(e) => {
                eprintln!("squint: {e}");
                EXIT_USAGE
            }
        },
        _ => {
            eprintln!("squint: call takes an action and --arguments, or --bundle");
            EXIT_USAGE
        }
    }
}

/// The directories a call's image paths are read from.
#[derive(Clone, Copy)]
enum Dirs<'a> {
    /// The path rule's (table-manners 5.2): each `--allow-dir`, narrowed by `[serve] allow_dirs`.
    Flags,
    /// A replay's own: the directory squint copied the bundle's input members into. squint chose
    /// it and wrote it, so it needs no `--allow-dir` (the "under an allowed directory" of 4.9).
    Own(&'a Path),
}

/// How a result is printed on stdout.
#[derive(Clone, Copy)]
enum Print {
    /// Indented, as one call prints it.
    Pretty,
    /// On one line, as each step of a bundle of several prints it.
    Line,
}

/// What one call ended with: its exit code, and whether it printed a `completed` result (a
/// bundle's replay goes on to its next step only then).
struct Step {
    code: i32,
    completed: bool,
}

impl Step {
    fn stopped(code: i32) -> Step {
        Step {
            code,
            completed: false,
        }
    }
}

/// Replays the bundle at `path` (table-manners 4.8, 4.9). The bundle path follows the path rule
/// with the directories `--allow-dir` gives (5.2), and the reader checks it whole (every member
/// listed and present, every input hashing to its `file_sha256`, every step a squint call at
/// 0.4.0 whose inputs are its invocation's images, nothing left out or unportable). A bundle
/// refused there is refused before any step: nothing runs and no run is written (no action is
/// known to write one under), the reason on stderr, exit 64. The input members are then copied
/// into `$XDG_CACHE_HOME/squint/bundles/<sha256 of invocation.json>/inputs/`, a directory only
/// squint writes, and each step is called in order through [`one`] with its arguments pointed
/// there. A single step prints its result and exits as a call does; several print one result per
/// line, stop at the first step that does not complete, and exit with that step's code; when every
/// step completes, with the most serious of their codes ([`most_serious`]), so an earlier `fail`
/// is not hidden by a later `pass`. squint has no operations, so no step names an earlier step's
/// output (4.9's binding by port does not arise): every image is a member of the bundle.
fn replay(args: &CallArgs, path: &Path) -> i32 {
    let refused = |reason: String| {
        eprintln!("squint: invalid (input): {reason}");
        EXIT_USAGE
    };
    let config = match Config::open(args.config.as_deref(), &args.overrides).require() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let allowed = match crate::serve::allowed_dirs(&args.allow_dirs, &config.serve.allow_dirs) {
        Ok(dirs) => dirs,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let real = match path.canonicalize() {
        Ok(real) if allowed.iter().any(|dir| real.starts_with(dir)) => real,
        _ => {
            return refused(format!(
                "--bundle {} is {}; give its directory with --allow-dir",
                path.display(),
                crate::serve::OUTSIDE
            ));
        }
    };
    let bundle = match Bundle::read(&real) {
        Ok(b) => b,
        Err(e) => return refused(e.to_string()),
    };
    let inputs = match place_inputs(&bundle) {
        Ok(dir) => dir,
        Err(e) => {
            eprintln!("squint: cannot place the bundle's inputs: {e}");
            return squint_core::run::EXIT_ERROR;
        }
    };
    let steps = bundle.manifest.steps.len();
    let print = if steps == 1 {
        Print::Pretty
    } else {
        Print::Line
    };
    let mut code = None;
    for (i, step) in bundle.manifest.steps.iter().enumerate() {
        let action = &step.invocation.action;
        let Some(call) = bundle.arguments(i, &inputs) else {
            return refused(format!("steps[{i}] has no arguments"));
        };
        if steps > 1 {
            eprintln!("step {}/{steps}: {action}", i + 1);
        }
        let done = one(args, action, call, Dirs::Own(&inputs), print);
        if !done.completed {
            return done.code;
        }
        code = Some(code.map_or(done.code, |c| most_serious(c, done.code)));
    }
    code.unwrap_or(EXIT_USAGE)
}

/// The more serious of two completed steps' exit codes (table-manners 6): an error (3) or
/// `invalid` (64) first, then `fail` (1), then `unsure` (2), then `pass` (0). Of two equally
/// serious codes the earlier stands; a code outside these ranks with the most serious.
fn most_serious(earlier: i32, later: i32) -> i32 {
    fn rank(code: i32) -> u8 {
        match code {
            0 => 0,
            2 => 1,
            1 => 2,
            _ => 3,
        }
    }
    if rank(later) > rank(earlier) {
        later
    } else {
        earlier
    }
}

/// Copies the bundle's input members into the directory squint keeps for it,
/// `$XDG_CACHE_HOME/squint/bundles/<sha256 of invocation.json>/inputs/`, and returns that
/// directory resolved. A file already there with the member's bytes is kept; any other is
/// replaced by a write to a temporary name and a rename, so a concurrent replay never reads a
/// half-written input.
fn place_inputs(bundle: &Bundle) -> Result<PathBuf, String> {
    let dir = crate::cache_home()?
        .join("squint")
        .join("bundles")
        .join(&bundle.manifest_sha256)
        .join("inputs");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for (name, bytes) in bundle.members() {
        let Some(file) = name.strip_prefix("inputs/") else {
            continue;
        };
        let target = dir.join(file);
        if std::fs::read(&target).is_ok_and(|held| held == bytes) {
            continue;
        }
        let temporary = dir.join(format!(".{file}.{}.tmp", std::process::id()));
        std::fs::write(&temporary, bytes).map_err(|e| format!("{}: {e}", temporary.display()))?;
        std::fs::rename(&temporary, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    dir.canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))
}

/// One call's run: a command-line run (no session, no `client_run_id`) with its run directory
/// and `run.json` like `check`; the result on stdout; the exit code of table-manners 6. What
/// stops it before a run (an unknown or reserved action, arguments that are not an object or that
/// `job_from_call` refuses as usage, no configuration, a `--allow-dir` that does not resolve, an
/// unmet requirement) is a usage error on stderr, exit 64. A path the rule refuses, or an image in
/// that is not one, is an `invalid` run, `input`, exit 64, as `serve` answers it.
fn one(
    args: &CallArgs,
    action: &str,
    call: Map<String, Value>,
    dirs: Dirs<'_>,
    print: Print,
) -> Step {
    let usage = |e: String| {
        eprintln!("squint: {e}");
        Step::stopped(EXIT_USAGE)
    };
    #[cfg(feature = "testkit")]
    let scripted = args.provider_script.is_some();
    #[cfg(not(feature = "testkit"))]
    let scripted = false;
    let described = match crate::describe::described(
        Config::open(args.config.as_deref(), &args.overrides),
        scripted,
    ) {
        Ok(d) => d,
        Err(e) => return usage(e.to_string()),
    };
    if contract::RESERVED_NAMES.contains(&action) {
        return usage(format!(
            "{action:?} is a name the contract reserves, not an action; squint describe --json \
             lists the actions"
        ));
    }
    if !described
        .description
        .actions
        .iter()
        .any(|a| a.name == action)
    {
        return usage(format!(
            "no action {action:?}; squint describe --json lists the actions"
        ));
    }
    let inspection = described.inspection;
    let mut config = match described.file.require() {
        Ok(c) => c,
        Err(e) => return usage(e.to_string()),
    };
    // The configuration as loaded, before `max_spend_usd` lowers its cap: the estimate's
    // identity, which the invocation records (table-manners 4.8).
    let config_identity = contract::config_identity(&config);
    let allowed = match dirs {
        Dirs::Flags => {
            match crate::serve::allowed_dirs(&args.allow_dirs, &config.serve.allow_dirs) {
                Ok(dirs) => dirs,
                Err(e) => return usage(e),
            }
        }
        Dirs::Own(dir) => vec![dir.to_path_buf()],
    };
    if let Err(e) = crate::admitted(&config, false) {
        return usage(e);
    }
    let tools = match crate::tools_for_config(&config, None, &Default::default()) {
        Ok(t) => t,
        Err(e) => return usage(e),
    };
    if let Err(e) = crate::check_wire_schemas(&config, &tools) {
        return usage(e.to_string());
    }
    // What a check needs, as a server gates it before its paths (table-manners 3.1).
    let needs = Needs {
        provider: true,
        tools: tools.manifests().map(|m| m.name.clone()).collect(),
    };
    let unmet = inspection.unmet(&Answers::default(), &needs);
    if !unmet.is_empty() {
        return usage(readiness::not_ready_message(&unmet));
    }
    let out_dir = args
        .out
        .clone()
        .unwrap_or_else(|| config.output.dir.clone());
    // A run refused before its decision was allocated, answered as `serve` answers it.
    let refused = |e: &squint_core::Error| -> Step {
        let association = association(action);
        match crate::serve::refused_run(
            &out_dir,
            action,
            &association,
            &call,
            Some(e),
            Map::new(),
            |_| {},
        ) {
            Ok(result) => printed(&result, None, print),
            Err(e) => {
                eprintln!("squint: {e}");
                Step::stopped(squint_core::run::EXIT_ERROR)
            }
        }
    };
    let mut job = match contract::job_from_call(&action.replace('_', "-"), &call) {
        Ok(job) => job,
        Err(squint_core::Error::Job(m)) => return usage(m),
        Err(e) => return refused(&e),
    };
    if let Err(e) = crate::serve::inside_allowed(&mut job, &allowed) {
        return refused(&e);
    }
    let max_spend_usd = match call.get("max_spend_usd") {
        None => None,
        Some(v) => match v.as_f64() {
            Some(max) if max > 0.0 => Some(max),
            _ => return usage(format!("max_spend_usd is {v}; it must be a number above 0")),
        },
    };
    if let Some(max) = max_spend_usd
        && let Err(e) = config.call_cap(max)
    {
        return refused(&e);
    }
    let opts = crate::ProviderOptions {
        replay: None,
        record: None,
        #[cfg(feature = "testkit")]
        provider_script: args.provider_script.as_deref(),
    };
    let provider = match crate::provider_for(&mut config, &opts) {
        Ok(p) => p,
        Err(m) => return usage(m),
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return usage(format!("cannot start the runtime: {e}")),
    };
    let cancel = Arc::new(Cancel::new());
    // The exit code of the signal that cancelled the run, 0 until one arrives; registered before
    // the run starts, as `check` does.
    let signalled = Arc::new(AtomicI32::new(0));
    let signals = {
        let _guard = runtime.enter();
        crate::install_signal_handlers()
    };
    runtime.spawn(crate::cancel_on_signal(
        signals,
        Arc::clone(&cancel),
        Arc::clone(&signalled),
    ));
    let allocated = Arc::new(std::sync::Mutex::new(None::<PathBuf>));
    let told = Arc::clone(&allocated);
    let services = Services::new(provider.as_ref(), &tools)
        .with_cancel(&cancel)
        .with_call(contract::CallFacts {
            config_identity: Some(config_identity),
            max_spend_usd,
            requirements: inspection.used(&needs),
        })
        .with_allocated(Arc::new(move |dir: &Path| {
            if let Ok(mut slot) = told.lock() {
                *slot = Some(dir.to_path_buf());
            }
        }));
    let outcome = runtime.block_on(squint_core::run::check(
        &config,
        &job,
        Path::new("."),
        &services,
        &out_dir,
        VERSION,
    ));
    let signal = || match signalled.load(Ordering::SeqCst) {
        0 => None,
        code => Some(code),
    };
    let decided = allocated.lock().ok().and_then(|slot| slot.clone());
    match (outcome, decided) {
        (Ok(o), _) => match read_result(&o.dir) {
            Ok(result) => {
                eprintln!("decision: {}", o.dir.display());
                // A signal that cancelled the decision before its verdict exits as signalled.
                let code = match signal() {
                    Some(code) if o.decision.verdict.is_none() => code,
                    _ => o.exit_code,
                };
                printed(&result, Some(code), print)
            }
            Err(e) => usage_or_error(&e),
        },
        // A failure after the decision was allocated is answered with its own run.
        (Err(e), Some(dir)) if dir.join(contract::RUN_FILE).exists() => match read_result(&dir) {
            Ok(result) => {
                eprintln!("decision: {}", dir.display());
                printed(&result, signal(), print)
            }
            Err(_) => usage_or_error(&e.to_string()),
        },
        // Cancelled before the decision was allocated: no run (table-manners 4.2).
        (Err(squint_core::Error::Cancelled(m)), None) => {
            eprintln!("squint: {m}");
            Step::stopped(signal().unwrap_or(EXIT_SIGINT))
        }
        (Err(e), _) => {
            let redacted = squint_core::artifacts::redact(&e.to_string(), &config.secrets);
            match signal() {
                Some(code) => {
                    eprintln!("squint: {redacted}");
                    Step::stopped(code)
                }
                None => refused(&e),
            }
        }
    }
}

/// The exit code of a run cancelled by SIGINT, when no signal was recorded.
const EXIT_SIGINT: i32 = 130;

/// A command-line run: no session, no `client_run_id`, listed under its action.
fn association(action: &str) -> contract::Association {
    contract::Association {
        session_id: None,
        client_run_id: None,
        action: Some(action.to_string()),
    }
}

/// The arguments file, read whole: a JSON object, as an MCP call's `arguments` are.
fn read_arguments(path: &Path) -> Result<Map<String, Value>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("--arguments {}: {e}", path.display()))?;
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(format!(
            "--arguments {}: the arguments are a JSON object, as an MCP call sends them",
            path.display()
        )),
        Err(e) => Err(format!("--arguments {}: {e}", path.display())),
    }
}

fn read_result(dir: &Path) -> Result<table_manners::RunResult, String> {
    let text = std::fs::read_to_string(dir.join(contract::RUN_FILE)).map_err(|e| e.to_string())?;
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn usage_or_error(message: &str) -> Step {
    eprintln!("squint: {message}");
    Step::stopped(squint_core::run::EXIT_ERROR)
}

/// Prints the result (table-manners 2) on stdout and returns its exit code (6): the decision's
/// own (`decided`: 0 pass, 1 fail, 2 unsure, 3 error, or the signal's for a cancelled run) when
/// there is one, else the status's: 64 for `invalid`, 3 for `error`, 130 for `cancelled`.
fn printed(result: &table_manners::RunResult, decided: Option<i32>, print: Print) -> Step {
    let text = match print {
        Print::Pretty => serde_json::to_string_pretty(result),
        Print::Line => serde_json::to_string(result),
    };
    match text {
        Ok(text) => println!("{text}"),
        Err(e) => return usage_or_error(&e.to_string()),
    }
    if let Some(e) = &result.error {
        eprintln!("{}: {}", result.action, e.reason);
    }
    let completed = result.status == table_manners::Status::Completed;
    Step {
        code: decided.unwrap_or(match result.status {
            table_manners::Status::Completed => 0,
            table_manners::Status::Invalid => EXIT_USAGE,
            table_manners::Status::Error => squint_core::run::EXIT_ERROR,
            table_manners::Status::Cancelled => EXIT_SIGINT,
        }),
        completed,
    }
}
