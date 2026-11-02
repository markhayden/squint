//! Task 1.11: the cache key flips for every key input and for no excluded one; a hit returns the
//! whole decision without a request; the cache is consulted once per decision; pruned artifacts
//! make a miss; an errored decision is never written; an evaluation never reads.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use squint_core::cache::{Cache, KeyInput};
use squint_core::config::Config;
use squint_core::job::Job;
use squint_core::report::Report;
use squint_core::run::{self, Services};
use squint_core::tool::ToolRegistry;
use squint_testkit::ScriptedProvider;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-cache-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(overrides: &[String]) -> Config {
    let text = std::fs::read_to_string(root().join("fixtures/config/minimal.toml")).unwrap();
    Config::parse(&text, overrides, &|_| None).unwrap()
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

struct Run {
    outcome: run::Outcome,
    provider: ScriptedProvider,
    script_len: usize,
}

impl Run {
    fn report(&self) -> Report {
        Report::read(&self.outcome.dir).unwrap()
    }
    fn runs_executed(&self) -> usize {
        (1..=2)
            .filter(|n| self.outcome.dir.join(format!("run-{n}")).exists())
            .count()
    }
}

fn check(out: &Path, cache_dir: &Path, script: &str, extra: &[&str], evaluation: bool) -> Run {
    let mut overrides = vec![format!("cache.dir={}", cache_dir.display())];
    overrides.extend(extra.iter().map(|s| s.to_string()));
    let config = config(&overrides);
    let text = std::fs::read_to_string(root().join("fixtures/scripts").join(script)).unwrap();
    let script_len = serde_json::from_str::<Vec<serde_json::Value>>(&text)
        .unwrap()
        .len();
    let provider = ScriptedProvider::from_json(&text).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let mut services = Services::new(&provider, &tools);
    services.evaluation = evaluation;
    let outcome = block_on(run::check(&config, &job, &root(), &services, out, "test")).unwrap();
    Run {
        outcome,
        provider,
        script_len,
    }
}

// ---- key -------------------------------------------------------------------------------------

fn key_input(config: &Config) -> KeyInput {
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let setup = run::setup(config, &job, &root(), &tools, None).unwrap();
    KeyInput::gather(
        config,
        &job,
        &setup,
        setup.image_plan().unwrap(),
        &tools,
        &squint_testkit::ScriptedProvider::from_json("[]").unwrap(),
        "test",
    )
}

#[test]
fn cache_key_changes_for_every_key_input() {
    let base = key_input(&config(&[])).key().unwrap();
    type Flip = Box<dyn Fn(&mut KeyInput)>;
    let flips: Vec<(&str, Flip)> = vec![
        ("use_case", Box::new(|k| k.use_case.push('x'))),
        ("brief", Box::new(|k| k.brief = Some("other".into()))),
        ("image sha", Box::new(|k| k.images[0].sha256.push('0'))),
        (
            "canonical sha",
            Box::new(|k| k.images[0].canonical_sha256.push('0')),
        ),
        (
            "image label",
            Box::new(|k| k.images[0].label = Some("relabelled".into())),
        ),
        (
            "prep version",
            Box::new(|k| k.images[0].prep_version.push('2')),
        ),
        (
            "rules version",
            Box::new(|k| {
                k.rules.versions.insert("source-fidelity".into(), 99);
            }),
        ),
        (
            "instances",
            Box::new(|k| k.instances.pop().map(drop).unwrap_or(())),
        ),
        (
            "checklist",
            Box::new(|k| k.checklist.push("chk:extra".into())),
        ),
        ("system prompt", Box::new(|k| k.system_prompt.push(' '))),
        (
            "schema version",
            Box::new(|k| {
                k.versions.insert("submission".into(), 2);
            }),
        ),
        ("adapter", Box::new(|k| k.adapter.push('2'))),
        ("profile", Box::new(|k| k.profile.push('2'))),
        ("model", Box::new(|k| k.model.push('2'))),
        (
            "controls",
            Box::new(|k| {
                k.controls
                    .insert("effort".into(), serde_json::json!("high"));
            }),
        ),
        ("max output", Box::new(|k| k.max_output_tokens += 1)),
        ("budget", Box::new(|k| k.budget.tool_turns += 1)),
        ("decision runs", Box::new(|k| k.budget.decision_runs = 2)),
        (
            "image settings",
            Box::new(|k| k.image_settings.max_long_edge = Some(512)),
        ),
        (
            "tools enabled",
            Box::new(|k| k.tools_enabled.push("regions".into())),
        ),
        (
            "tools order",
            Box::new(|k| k.tools_order.push("regions".into())),
        ),
        ("harness version", Box::new(|k| k.harness_version.push('1'))),
    ];
    let mut seen = std::collections::BTreeSet::new();
    for (name, flip) in flips {
        let mut k = key_input(&config(&[]));
        flip(&mut k);
        let key = k.key().unwrap();
        assert_ne!(key, base, "{name} is part of the key");
        assert!(
            seen.insert(key),
            "{name} produced a key another flip produced"
        );
    }
}

#[test]
fn cache_key_changes_with_a_packs_lock_digest() {
    // The same tools from a pack whose lock pins other dependency versions are other tools: the
    // lock digest the installed bundle carries is part of the key (SPEC "Python tool pack").
    let config = config(&[]);
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let key_with = |digest: &str| {
        let tools = ToolRegistry::new(vec![], &[], &[])
            .unwrap()
            .with_lock_digests(std::collections::BTreeMap::from([(
                "regions".to_string(),
                digest.to_string(),
            )]));
        let setup = run::setup(&config, &job, &root(), &tools, None).unwrap();
        KeyInput::gather(
            &config,
            &job,
            &setup,
            setup.image_plan().unwrap(),
            &tools,
            &squint_testkit::ScriptedProvider::from_json("[]").unwrap(),
            "test",
        )
        .key()
        .unwrap()
    };
    let base = key_input(&config).key().unwrap();
    assert_ne!(key_with("a"), base);
    assert_ne!(key_with("a"), key_with("b"));
    assert_eq!(key_with("a"), key_with("a"));
}

#[test]
fn cache_key_ignores_credentials_output_and_cache_settings() {
    let base = key_input(&config(&[])).key().unwrap();
    for overrides in [
        vec!["output.dir=/elsewhere".to_string()],
        vec![
            "cache.dir=/tmp/other-cache".to_string(),
            "cache.read=false".to_string(),
        ],
        vec!["privacy.account=someone-else".to_string()],
        vec!["policy.on_unsure=\"pass\"".to_string()],
        vec!["tools.packs.fake.command=\"/elsewhere/server\"".to_string()],
    ] {
        let mut config = config(&overrides);
        config.secrets.push("SYNTHETIC-secret".into());
        assert_eq!(
            key_input(&config).key().unwrap(),
            base,
            "{overrides:?} is excluded"
        );
    }
}

// ---- decision boundary -----------------------------------------------------------------------

#[test]
fn cache_hit_returns_the_whole_decision_without_a_request() {
    let cache_dir = tempdir("hit-cache");
    let first = check(&tempdir("hit-1"), &cache_dir, "held.json", &[], false);
    assert_eq!(first.outcome.exit_code, 0, "{:?}", first.outcome.error);
    assert_eq!(first.report().cache.status, "miss");
    assert_eq!(first.provider.remaining(), 0);

    let second = check(&tempdir("hit-2"), &cache_dir, "held.json", &[], false);
    assert_eq!(second.outcome.exit_code, 0);
    let r = second.report();
    assert_eq!(r.cache.status, "hit");
    assert_eq!(
        r.cache.source.as_deref().unwrap().canonicalize().unwrap(),
        first.outcome.dir.canonicalize().unwrap()
    );
    assert_eq!(r.cache.key, first.report().cache.key);
    assert_eq!(
        second.provider.remaining(),
        second.script_len,
        "no request was made"
    );
    assert_eq!(second.runs_executed(), 0, "no run of its own");
    assert_eq!(r.verdict, first.report().verdict);
    assert_eq!(r.runs.len(), 1, "the cached decision's runs come along");
}

#[test]
fn cache_is_consulted_once_per_decision_not_per_run() {
    let cache_dir = tempdir("two-runs-cache");
    let first = check(
        &tempdir("two-runs-1"),
        &cache_dir,
        "two-runs-agree.json",
        &["budget.decision_runs=2"],
        false,
    );
    assert_eq!(first.outcome.exit_code, 0, "{:?}", first.outcome.error);
    assert_eq!(
        first.runs_executed(),
        2,
        "the second run inside the decision never hits"
    );
    assert_eq!(first.provider.remaining(), 0);
    let second = check(
        &tempdir("two-runs-2"),
        &cache_dir,
        "two-runs-agree.json",
        &["budget.decision_runs=2"],
        false,
    );
    assert_eq!(second.report().cache.status, "hit");
    assert_eq!(second.runs_executed(), 0);
    assert_eq!(second.report().runs.len(), 2);
}

#[test]
fn cache_pruned_artifacts_make_a_miss() {
    let cache_dir = tempdir("pruned-cache");
    let first = check(&tempdir("pruned-1"), &cache_dir, "held.json", &[], false);
    std::fs::remove_dir_all(first.outcome.dir.join("run-1/artifacts")).unwrap();
    let second = check(&tempdir("pruned-2"), &cache_dir, "held.json", &[], false);
    assert_eq!(second.report().cache.status, "miss");
    assert_eq!(second.runs_executed(), 1);
    let third = check(&tempdir("pruned-3"), &cache_dir, "held.json", &[], false);
    assert_eq!(
        third.report().cache.status,
        "hit",
        "the fresh decision replaced the stale entry"
    );
    assert_eq!(
        third
            .report()
            .cache
            .source
            .as_deref()
            .unwrap()
            .canonicalize()
            .unwrap(),
        second.outcome.dir.canonicalize().unwrap()
    );
}

#[test]
fn cache_is_never_written_after_an_error() {
    let cache_dir = tempdir("error-cache");
    let first = check(
        &tempdir("error-1"),
        &cache_dir,
        "missing-item.json",
        &[],
        false,
    );
    assert_eq!(first.outcome.exit_code, 3);
    let second = check(
        &tempdir("error-2"),
        &cache_dir,
        "missing-item.json",
        &[],
        false,
    );
    assert_eq!(second.report().cache.status, "miss");
    assert_eq!(second.runs_executed(), 1);
    let key = second.report().cache.key.unwrap();
    assert!(
        Cache::open(&cache_dir)
            .unwrap()
            .lookup(&key)
            .unwrap()
            .is_none()
    );
}

#[test]
fn cache_is_bypassed_by_an_evaluation() {
    let cache_dir = tempdir("eval-cache");
    let first = check(&tempdir("eval-1"), &cache_dir, "held.json", &[], false);
    assert_eq!(first.report().cache.status, "miss");
    let eval = check(&tempdir("eval-2"), &cache_dir, "held.json", &[], true);
    assert_eq!(eval.report().cache.status, "bypass");
    assert_eq!(
        eval.runs_executed(),
        1,
        "an evaluation never reads the cache"
    );
}

#[test]
fn cache_is_off_without_a_dir_and_with_read_off() {
    let out = tempdir("off");
    let provider = ScriptedProvider::from_path(&root().join("fixtures/scripts/held.json")).unwrap();
    let tools = ToolRegistry::new(vec![], &[], &[]).unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = block_on(run::check(
        &config(&[]),
        &job,
        &root(),
        &Services::new(&provider, &tools),
        &out,
        "test",
    ))
    .unwrap();
    let r = Report::read(&o.dir).unwrap();
    assert_eq!(r.cache.status, "off");
    assert!(r.cache.key.is_none());

    let cache_dir = tempdir("read-off-cache");
    let first = check(&tempdir("read-off-1"), &cache_dir, "held.json", &[], false);
    assert_eq!(first.report().cache.status, "miss");
    let second = check(
        &tempdir("read-off-2"),
        &cache_dir,
        "held.json",
        &["cache.read=false"],
        false,
    );
    assert_eq!(
        second.report().cache.status,
        "miss",
        "read off: the entry is ignored"
    );
    assert_eq!(second.runs_executed(), 1);
}

#[test]
fn cache_write_off_leaves_no_entry() {
    let cache_dir = tempdir("write-off-cache");
    let first = check(
        &tempdir("write-off-1"),
        &cache_dir,
        "held.json",
        &["cache.write=false"],
        false,
    );
    assert_eq!(first.report().cache.status, "miss");
    let second = check(&tempdir("write-off-2"), &cache_dir, "held.json", &[], false);
    assert_eq!(second.report().cache.status, "miss", "nothing was stored");
    assert_eq!(second.runs_executed(), 1);
}

#[test]
fn cache_is_not_written_when_one_of_two_runs_errors() {
    let cache_dir = tempdir("two-run-error-cache");
    let first = check(
        &tempdir("two-run-error-1"),
        &cache_dir,
        "two-runs-pass-then-error.json",
        &["budget.decision_runs=2"],
        false,
    );
    assert_eq!(first.outcome.exit_code, 3);
    assert_eq!(first.runs_executed(), 2);
    let key = first.report().cache.key.unwrap();
    assert!(
        Cache::open(&cache_dir)
            .unwrap()
            .lookup(&key)
            .unwrap()
            .is_none()
    );
}

#[test]
fn cache_hit_applies_this_decisions_policy_to_the_exit_code() {
    let cache_dir = tempdir("policy-cache");
    let first = check(
        &tempdir("policy-1"),
        &cache_dir,
        "two-runs-pass-then-unsure.json",
        &["budget.decision_runs=2"],
        false,
    );
    assert_eq!(first.outcome.exit_code, 2, "{:?}", first.outcome.error);
    let second = check(
        &tempdir("policy-2"),
        &cache_dir,
        "two-runs-pass-then-unsure.json",
        &["budget.decision_runs=2", "policy.on_unsure=\"fail\""],
        false,
    );
    let r = second.report();
    assert_eq!(r.cache.status, "hit");
    assert_eq!(
        second.outcome.exit_code, 1,
        "the hit is judged under this check's policy"
    );
    assert_eq!(r.exit_code, 1);
    assert_eq!(r.verdict, first.report().verdict);
}
