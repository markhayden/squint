//! Task 1.3: the full config example parses; the committed JSON Schema matches the generated one.
#![allow(clippy::unwrap_used)]

use squint_core::config::Config;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn every_key_in_the_spec_example_parses() {
    let text = std::fs::read_to_string(root().join("fixtures/config/full.toml")).unwrap();
    let env = |name: &str| (name == "SQUINT_TEST_ACCOUNT").then(|| "acct".to_string());
    let config = Config::parse(&text, &[], &env).unwrap();
    assert_eq!(
        config.tools.packs["python"].package.as_deref(),
        Some("squint-tools")
    );
    assert_eq!(config.rules.as_ref().unwrap().base, "source-fidelity");
    assert_eq!(config.images.max_long_edge, Some(1568));
    assert_eq!(config.provider.controls["effort"].as_str(), Some("high"));
    assert_eq!(config.serve.concurrency, 2);
    assert_eq!(config.serve.session_budget_usd, 5.0);
}

#[test]
fn committed_config_schema_matches_the_generated_one() {
    let generated = serde_json::to_string_pretty(&Config::json_schema()).unwrap() + "\n";
    let path = root().join("schemas/config-v1.json");
    if std::env::var_os("UPDATE_SCHEMAS").is_some() {
        std::fs::write(&path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        committed, generated,
        "schemas/config-v1.json is stale; run with UPDATE_SCHEMAS=1 to regenerate"
    );
}
