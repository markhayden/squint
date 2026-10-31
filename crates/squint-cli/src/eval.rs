//! `squint eval`: the matrix runner (3.4b), the estimate (3.4c), the report (3.5a), the gate
//! lifecycle (3.5b), the canary and requalification (3.7), and the gate table on a statistics
//! fixture (1.15).

use std::path::{Path, PathBuf};

use clap::Subcommand;
use squint_core::config::Config;
use squint_core::provider::Provider;
use squint_core::run::Cancel;
use squint_core::tool::ToolRegistry;
use squint_eval::admissions::Admissions;
use squint_eval::arms::ToolSelection;
use squint_eval::fixture::StatsFixture;
use squint_eval::gate::{self, GateStage};
use squint_eval::matrix::{Cell, Matrix};
use squint_eval::runner::{self, Factories, Plan};

use crate::{EXIT_USAGE, ProviderOptions, VERSION};

#[derive(clap::Args)]
pub struct EvalArgs {
    #[command(subcommand)]
    pub command: EvalCommand,
}

#[derive(Subcommand)]
pub enum EvalCommand {
    /// Run (or resume) a matrix: every item of its sets through every cell, never reading the
    /// cache, under the evaluation's durable ledger.
    Run(RunArgs),
    /// Print every decision's envelope, their sum and a cost range for a matrix, and warn when
    /// the sum exceeds the cap; nothing is sent and nothing is refused.
    Estimate(EstimateArgs),
    /// Regenerate `metrics.json` and `report.md` in an evaluation directory from its saved
    /// results and the sets' labels; nothing is sent.
    Report(ReportArgs),
    /// Score one hypothesis (one tool, one model, one sealed set) from a finished evaluation
    /// directory: verify the seal and the descriptor, write the receipt, publish a blind set's
    /// labels for the owner to review and commit, and record the admission.
    Gate(GateArgs),
    /// Apply the gate table to a synthetic result table and print the interval and outcome.
    GateTable(GateTableArgs),
    /// Rerun the pinned canary set, uncached, under the configuration and compare every verdict
    /// with the recorded baseline: clear, suspect, or recommend_disable. Never disables.
    Canary(CanaryArgs),
    /// Rerun the full set under the configuration and, with --approve, record it as the new
    /// baseline (clearing the canary history); without it, show what would change.
    Requalify(CanaryArgs),
}

#[derive(clap::Args)]
pub struct CanaryArgs {
    /// The configuration as deployed.
    #[arg(long)]
    config: PathBuf,
    /// The pinned set (`<use-case>/<set>`).
    #[arg(long)]
    set: String,
    /// The canary state file (the baseline and every run); created by `requalify --approve`.
    #[arg(long)]
    state: PathBuf,
    /// The evalsets root the set lives under.
    #[arg(long, default_value = "evalsets")]
    evalsets_root: PathBuf,
    /// Where the decisions go.
    #[arg(long, default_value = "eval-out/canary")]
    out: PathBuf,
    /// `requalify` only: the owner's words of approval; without them nothing is recorded.
    #[arg(long)]
    approve: Option<String>,
    /// The most the whole run may spend; it refuses before sending anything it cannot cover.
    #[arg(long, default_value_t = 25.0)]
    spend_cap_usd: f64,
    /// Print JSON instead of the text form.
    #[arg(long)]
    json: bool,
    /// Scripted fake provider, for tests and debugging only.
    #[cfg(feature = "testkit")]
    #[arg(long)]
    provider_script: Option<PathBuf>,
    /// Scripted envelopes for the fake `fake_regions` tool, for tests and debugging only.
    #[cfg(feature = "testkit")]
    #[arg(long)]
    tool_script: Option<PathBuf>,
}

struct CliJudge {
    #[cfg(feature = "testkit")]
    provider_script: Option<PathBuf>,
    #[cfg(feature = "testkit")]
    tool_script: Option<PathBuf>,
}

impl squint_eval::canary::Judge for CliJudge {
    fn provider(&self, config: &Config) -> squint_eval::Result<Box<dyn Provider>> {
        let mut config = config.clone();
        let opts = ProviderOptions {
            #[cfg(feature = "testkit")]
            provider_script: self.provider_script.as_deref(),
            ..ProviderOptions::default()
        };
        crate::provider_for(&mut config, &opts).map_err(squint_eval::Error::Canary)
    }

    fn tools(&self, config: &Config) -> squint_eval::Result<ToolRegistry> {
        #[cfg(feature = "testkit")]
        let script = self.tool_script.as_deref();
        #[cfg(not(feature = "testkit"))]
        let script = None;
        crate::tools_for_config(config, script, &crate::SharedPacks::default())
            .map_err(squint_eval::Error::Canary)
    }
}

fn canary_setup(args: &CanaryArgs) -> Result<(Config, squint_eval::sets::Set, CliJudge), String> {
    let config = Config::load(&args.config).map_err(|e| e.to_string())?;
    let set = squint_eval::sets::Set::load(&args.evalsets_root.join(&args.set))
        .map_err(|e| e.to_string())?;
    Ok((
        config,
        set,
        CliJudge {
            #[cfg(feature = "testkit")]
            provider_script: args.provider_script.clone(),
            #[cfg(feature = "testkit")]
            tool_script: args.tool_script.clone(),
        },
    ))
}

fn eval_canary(args: &CanaryArgs) -> i32 {
    let (config, set, judge) = match canary_setup(args) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let setup = squint_eval::canary::Setup {
        config: &config,
        set: &set,
        out_dir: &args.out,
        judge: &judge,
        harness_version: VERSION,
        spend_cap_usd: args.spend_cap_usd,
    };
    match squint_eval::canary::canary(&args.state, &setup) {
        Ok(run) => {
            if args.json {
                println!("{}", serde_json::to_string_pretty(&run).unwrap_or_default());
            } else {
                println!(
                    "canary {} against baseline {}: {} ({} deviation(s), ${:.4})",
                    args.set,
                    run.baseline,
                    run.status,
                    run.deviations.len(),
                    run.cost_usd
                );
                for d in &run.deviations {
                    println!(
                        "  {}: baseline {}, observed {}",
                        d.item, d.baseline, d.observed
                    );
                }
                for item in &run.errors {
                    println!("  {item}: operational error, compared with nothing");
                }
                if run.status == squint_eval::canary::Status::RecommendDisable {
                    println!(
                        "two consecutive deviations: disabling this configuration is recommended; nothing was disabled, that is the owner's call"
                    );
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

fn eval_requalify(args: &CanaryArgs) -> i32 {
    let (config, set, judge) = match canary_setup(args) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let setup = squint_eval::canary::Setup {
        config: &config,
        set: &set,
        out_dir: &args.out,
        judge: &judge,
        harness_version: VERSION,
        spend_cap_usd: args.spend_cap_usd,
    };
    match squint_eval::canary::requalify(&args.state, &setup, args.approve.as_deref()) {
        Ok(q) => {
            if args.json {
                println!("{}", serde_json::to_string_pretty(&q).unwrap_or_default());
            } else {
                println!(
                    "requalify {}: baseline {} over {} items, ${:.4}{}",
                    args.set,
                    q.baseline.id,
                    q.baseline.verdicts.len(),
                    q.baseline.cost_usd,
                    if q.recorded {
                        format!(", recorded in {}", args.state.display())
                    } else {
                        "; not recorded (pass --approve \"<words>\" to record it)".into()
                    }
                );
                for d in &q.changed {
                    println!("  {}: was {}, now {}", d.item, d.baseline, d.observed);
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

#[derive(clap::Args)]
pub struct ReportArgs {
    /// The evaluation directory an `eval run` wrote.
    #[arg(long)]
    out: PathBuf,
    /// The evalsets root the summary's set ids resolve under.
    #[arg(long, default_value = "evalsets")]
    evalsets_root: PathBuf,
    /// Print the Markdown for these sets only (comma separated) to stdout and write nothing.
    #[arg(long, value_delimiter = ',')]
    sets: Vec<String>,
}

fn eval_report(args: &ReportArgs) -> i32 {
    if !args.sets.is_empty() {
        return match squint_eval::summary::render_subset(&args.out, &args.evalsets_root, &args.sets)
        {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(e) => {
                eprintln!("squint: {e}");
                EXIT_USAGE
            }
        };
    }
    match squint_eval::summary::regenerate(&args.out, &args.evalsets_root) {
        Ok(m) => {
            println!(
                "eval report: {} cells over labelled items, written to {} and {} in {}",
                m.cells.len(),
                squint_eval::summary::METRICS_FILE,
                squint_eval::summary::REPORT_FILE,
                args.out.display()
            );
            0
        }
        Err(e) => {
            eprintln!("squint: {e}");
            EXIT_USAGE
        }
    }
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// The matrix file.
    #[arg(long)]
    matrix: PathBuf,
    /// The evaluation directory: descriptor, ledger, results and summary.
    #[arg(long)]
    out: PathBuf,
    /// The evalsets root the matrix's set ids resolve under.
    #[arg(long, default_value = "evalsets")]
    evalsets_root: PathBuf,
    /// The admission record arm D reads; defaults to `<evalsets root>/admissions.toml` when it
    /// exists.
    #[arg(long)]
    admissions: Option<PathBuf>,
    /// Scripted fake provider responses for every decision, for tests and debugging only.
    #[cfg(feature = "testkit")]
    #[arg(long)]
    provider_script: Option<PathBuf>,
    /// Scripted envelopes for the fake `fake_regions` tool, for tests and debugging only.
    #[cfg(feature = "testkit")]
    #[arg(long)]
    tool_script: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct EstimateArgs {
    /// The matrix file.
    #[arg(long)]
    matrix: PathBuf,
    /// The evalsets root the matrix's set ids resolve under.
    #[arg(long, default_value = "evalsets")]
    evalsets_root: PathBuf,
    /// The admission record arm D reads; defaults to `<evalsets root>/admissions.toml` when it
    /// exists.
    #[arg(long)]
    admissions: Option<PathBuf>,
    /// Print JSON instead of the text form.
    #[arg(long)]
    json: bool,
    /// Scripted fake provider, for tests and debugging only.
    #[cfg(feature = "testkit")]
    #[arg(long)]
    provider_script: Option<PathBuf>,
    /// Scripted envelopes for the fake `fake_regions` tool, for tests and debugging only.
    #[cfg(feature = "testkit")]
    #[arg(long)]
    tool_script: Option<PathBuf>,
}

/// The plan a matrix file describes, with its config and admission record loaded; `Err` holds
/// the usage message.
fn plan_from(
    matrix_path: &Path,
    out: &Path,
    evalsets_root: &Path,
    admissions: Option<&Path>,
) -> Result<Plan, String> {
    let matrix_text = std::fs::read_to_string(matrix_path)
        .map_err(|e| format!("cannot read {}: {e}", matrix_path.display()))?;
    let matrix =
        Matrix::parse(&matrix_text).map_err(|e| format!("{}: {e}", matrix_path.display()))?;
    let config_text = std::fs::read_to_string(&matrix.matrix.config).map_err(|e| {
        format!(
            "cannot read the matrix's config {}: {e}",
            matrix.matrix.config.display()
        )
    })?;
    let admissions_path = admissions.map(Path::to_path_buf).or_else(|| {
        let p = evalsets_root.join("admissions.toml");
        p.exists().then_some(p)
    });
    let admissions = match admissions_path.as_deref().map(Admissions::load) {
        Some(Ok(a)) => Some(a),
        Some(Err(e)) => return Err(e.to_string()),
        None => None,
    };
    Ok(Plan {
        eval_dir: out.to_path_buf(),
        matrix,
        matrix_text,
        config_text,
        evalsets_root: evalsets_root.to_path_buf(),
        admissions,
        harness_version: VERSION.to_string(),
        allow_fake: cfg!(feature = "testkit"),
    })
}

fn eval_estimate(args: &EstimateArgs) -> i32 {
    let plan = match plan_from(
        &args.matrix,
        Path::new("."),
        &args.evalsets_root,
        args.admissions.as_deref(),
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let factories = CliFactories {
        #[cfg(feature = "testkit")]
        provider_script: args.provider_script.clone(),
        #[cfg(feature = "testkit")]
        tool_script: args.tool_script.clone(),
        lock_digests: Default::default(),
        shared_packs: Default::default(),
    };
    let e = match squint_eval::estimate::estimate(&plan, &factories) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("squint: {err}");
            return EXIT_USAGE;
        }
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&e).unwrap_or_default());
    } else {
        println!(
            "eval estimate {}: {} decisions",
            e.matrix,
            e.decisions.len()
        );
        for d in &e.decisions {
            println!("  {:<70} envelope ${:.4}", d.id, d.envelope_usd);
        }
        println!(
            "envelope sum ${:.4}; cost range ${:.4} (every decision submits at once) to ${:.4} (every envelope spent); cap ${:.2}",
            e.envelope_sum_usd, e.floor_usd, e.envelope_sum_usd, e.cap_usd
        );
        if e.over_cap {
            println!(
                "warning: the envelope sum exceeds the cap by ${:.4}; completion is not guaranteed (admission is per decision, against what is left)",
                e.envelope_sum_usd - e.cap_usd
            );
        }
        if !e.over_decision_cap.is_empty() {
            println!(
                "warning: {} of {} decisions have an envelope above the config's per-decision cap (budget.spend_cap_usd) and would be refused with error: budget; raise the cap in the matrix's config, for example {}",
                e.over_decision_cap.len(),
                e.decisions.len(),
                e.over_decision_cap[0]
            );
        }
    }
    0
}

/// The binary's providers and tools, per cell. The pack lock digests of the last registry built
/// are kept for the descriptor, which asks for them right after the tools.
struct CliFactories {
    #[cfg(feature = "testkit")]
    provider_script: Option<PathBuf>,
    #[cfg(feature = "testkit")]
    tool_script: Option<PathBuf>,
    lock_digests: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    /// Packs configured `shared = true`, one process for the whole evaluation.
    shared_packs: crate::SharedPacks,
}

impl Factories for CliFactories {
    fn provider(&self, _cell: &Cell, config: &Config) -> squint_eval::Result<Box<dyn Provider>> {
        let mut config = config.clone();
        let opts = ProviderOptions {
            #[cfg(feature = "testkit")]
            provider_script: self.provider_script.as_deref(),
            ..ProviderOptions::default()
        };
        crate::provider_for(&mut config, &opts).map_err(squint_eval::Error::Runner)
    }

    fn tools(
        &self,
        selection: &ToolSelection,
        config: &Config,
    ) -> squint_eval::Result<ToolRegistry> {
        #[cfg(feature = "testkit")]
        let script = self.tool_script.as_deref();
        #[cfg(not(feature = "testkit"))]
        let script = None;
        let registry = crate::tools_for_config(config, script, &self.shared_packs)
            .map_err(squint_eval::Error::Runner)?;
        if let Ok(mut held) = self.lock_digests.lock() {
            *held = registry.lock_digests().clone();
        }
        // The arm's projections (`align` under `visual_only` in the control arms) wrap the tools
        // here, so every envelope the run sees is already projected.
        registry
            .with_projections(&selection.projections)
            .map_err(|e| squint_eval::Error::Runner(e.to_string()))
    }

    fn lock_digests(&self, _config: &Config) -> std::collections::BTreeMap<String, String> {
        self.lock_digests
            .lock()
            .map(|d| d.clone())
            .unwrap_or_default()
    }
}

fn eval_run(args: &RunArgs) -> i32 {
    let plan = match plan_from(
        &args.matrix,
        &args.out,
        &args.evalsets_root,
        args.admissions.as_deref(),
    ) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let factories = CliFactories {
        #[cfg(feature = "testkit")]
        provider_script: args.provider_script.clone(),
        #[cfg(feature = "testkit")]
        tool_script: args.tool_script.clone(),
        lock_digests: Default::default(),
        shared_packs: Default::default(),
    };
    // Signals cancel between decisions and inside one; the runtime that listens stays alive for
    // the whole run while every decision runs on its own.
    let signals_runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
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
    let signalled = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(0));
    let signals = {
        let _guard = signals_runtime.enter();
        crate::install_signal_handlers()
    };
    signals_runtime.spawn(crate::cancel_on_signal(
        signals,
        std::sync::Arc::clone(&cancel),
        std::sync::Arc::clone(&signalled),
    ));
    match runner::run(&plan, &factories, Some(&cancel)) {
        Ok(s) => {
            let errors = s
                .decisions
                .iter()
                .filter(|d| d.error_class.is_some())
                .count();
            println!(
                "eval run {}: {} decisions ({} completed now, {} already complete, {} partial redone, {} errors), ${:.4} of ${:.2} spent, results in {}",
                s.matrix,
                s.decisions.len(),
                s.completed,
                s.skipped,
                s.partial.len(),
                errors,
                s.spent_usd,
                s.cap_usd,
                args.out.display()
            );
            if s.cancelled {
                eprintln!("squint: cancelled; run the same command again to resume");
                match signalled.load(std::sync::atomic::Ordering::SeqCst) {
                    0 => squint_core::run::EXIT_ERROR,
                    code => code,
                }
            } else if s.budget_exhausted {
                eprintln!(
                    "squint: the ${:.2} cap no longer covers the next decision's envelope; the run stopped there and holds no result for it. Resuming tries again; a larger cap is a new matrix and a new evaluation directory",
                    s.cap_usd
                );
                squint_core::run::EXIT_ERROR
            } else {
                0
            }
        }
        Err(e) => {
            eprintln!("squint: {e}");
            EXIT_USAGE
        }
    }
}

#[derive(clap::Args)]
pub struct GateTableArgs {
    /// A statistics fixture (`fixtures/stats/case-1.toml`).
    #[arg(long)]
    fixture: PathBuf,
    /// Print JSON instead of the text form.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
pub struct GateArgs {
    /// The matrix the evaluation was run from; its `[matrix.gate]` section freezes the gate.
    #[arg(long)]
    matrix: PathBuf,
    /// The evaluation directory `eval run` finished.
    #[arg(long)]
    out: PathBuf,
    /// The tool under test (arm `D_<tool>` against arm `C`).
    #[arg(long)]
    tool: String,
    /// The sealed set to score (`<use-case>/<set>`).
    #[arg(long)]
    set: String,
    /// `initial` (a blind set of at least 20 items) or `confirmation` (a distinct set of at least
    /// 50 items from families disjoint with every earlier set, under the identical judging
    /// configuration).
    #[arg(long)]
    stage: GateStage,
    /// `<profile>:<model>`; needed when the matrix has several models.
    #[arg(long)]
    model: Option<String>,
    /// The owner-held label file of a blind set.
    #[arg(long)]
    labels: Option<PathBuf>,
    /// The initial stage's receipt id, required by the confirmation stage.
    #[arg(long)]
    initial_receipt: Option<String>,
    /// An ADR reference permitting a set to be scored again.
    #[arg(long)]
    adr: Option<String>,
    /// The evalsets root the set lives under; receipts go to `<root>/receipts/`.
    #[arg(long, default_value = "evalsets")]
    evalsets_root: PathBuf,
    /// The admission record to update; defaults to `<evalsets root>/admissions.toml`.
    #[arg(long)]
    admissions: Option<PathBuf>,
    /// Print JSON instead of the text form.
    #[arg(long)]
    json: bool,
}

fn eval_gate(args: &GateArgs) -> i32 {
    let matrix = match Matrix::load(&args.matrix) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    let admissions = args
        .admissions
        .clone()
        .unwrap_or_else(|| args.evalsets_root.join("admissions.toml"));
    let report = gate::gate(&gate::Request {
        eval_dir: &args.out,
        matrix: &matrix,
        evalsets_root: &args.evalsets_root,
        set: &args.set,
        tool: &args.tool,
        model: args.model.as_deref(),
        stage: args.stage,
        labels: args.labels.as_deref(),
        initial_receipt: args.initial_receipt.as_deref(),
        adr: args.adr.as_deref(),
        admissions: &admissions,
    });
    let r = match report {
        Ok(r) => r,
        Err(e) => {
            eprintln!("squint: {e}");
            return EXIT_USAGE;
        }
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
        return 0;
    }
    let b = &r.receipt.receipt;
    println!(
        "gate {} stage {} for {} on {} ({} items): {}{}",
        b.tool,
        b.stage,
        b.model,
        b.set,
        b.items,
        serde_json::to_string(&b.outcome)
            .unwrap_or_default()
            .trim_matches('"'),
        if r.resumed {
            " (resumed from the receipt)"
        } else {
            ""
        }
    );
    println!(
        "C: detected {}/{} missed {} false rejects {} unsure {} errors {} cost ${:.4}",
        b.c.detected,
        b.c.expected_fail,
        b.c.missed,
        b.c.false_rejects,
        b.c.unsure,
        b.c.errors,
        b.c.cost_usd
    );
    println!(
        "X: detected {}/{} missed {} false rejects {} unsure {} errors {} cost ${:.4}",
        b.x.detected,
        b.x.expected_fail,
        b.x.missed,
        b.x.false_rejects,
        b.x.unsure,
        b.x.errors,
        b.x.cost_usd
    );
    let bs = &b.bootstrap;
    println!(
        "paired bootstrap: point {:.4}, interval [{:.4}, {:.4}], {} resamples, seed {}, {} of {} families differ{}",
        bs.point,
        bs.lower,
        bs.upper,
        bs.resamples,
        bs.seed,
        bs.differing_families,
        bs.families,
        if bs.insufficient_tail {
            " (tail unresolved)"
        } else {
            ""
        }
    );
    for reason in &b.failed {
        println!("gate not held: {reason}");
    }
    println!("receipt {}", b.id);
    match r.admission {
        Some(stage) => println!(
            "admission: {}",
            serde_json::to_string(&stage)
                .unwrap_or_default()
                .trim_matches('"')
        ),
        None => println!("admission: unchanged"),
    }
    for path in &r.written {
        println!("wrote {}", path.display());
    }
    println!("review and commit the written files; the gate never commits");
    0
}

pub fn run(args: &EvalArgs) -> i32 {
    match &args.command {
        EvalCommand::Run(a) => eval_run(a),
        EvalCommand::Estimate(a) => eval_estimate(a),
        EvalCommand::Report(a) => eval_report(a),
        EvalCommand::Gate(a) => eval_gate(a),
        EvalCommand::GateTable(a) => gate_table(a),
        EvalCommand::Canary(a) => eval_canary(a),
        EvalCommand::Requalify(a) => eval_requalify(a),
    }
}

fn gate_table(args: &GateTableArgs) -> i32 {
    let evaluation = StatsFixture::load(&args.fixture).and_then(|f| f.evaluate());
    let e = match evaluation {
        Ok(e) => e,
        Err(err) => {
            eprintln!("squint: {err}");
            return EXIT_USAGE;
        }
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&e).unwrap_or_default());
        return 0;
    }
    let b = &e.bootstrap;
    println!(
        "C: detected {}/{} missed {} false rejects {} unsure {} errors {} cost ${:.2}",
        e.c.detected,
        e.c.expected_fail,
        e.c.missed,
        e.c.false_rejects,
        e.c.unsure,
        e.c.errors,
        e.c.cost_usd
    );
    println!(
        "X: detected {}/{} missed {} false rejects {} unsure {} errors {} cost ${:.2}",
        e.x.detected,
        e.x.expected_fail,
        e.x.missed,
        e.x.false_rejects,
        e.x.unsure,
        e.x.errors,
        e.x.cost_usd
    );
    println!(
        "paired bootstrap: point {:.4}, interval [{:.4}, {:.4}] at percentiles {} and {}, {} resamples, seed {}, {} of {} families differ{}",
        b.point,
        b.lower,
        b.upper,
        b.lower_percentile,
        b.upper_percentile,
        b.resamples,
        b.seed,
        b.differing_families,
        b.families,
        if b.insufficient_tail {
            " (tail unresolved)"
        } else {
            ""
        }
    );
    for reason in &e.verdict.failed {
        println!("gate not held: {reason}");
    }
    println!(
        "outcome: {}",
        serde_json::to_string(&e.verdict.outcome)
            .unwrap_or_default()
            .trim_matches('"')
    );
    0
}
