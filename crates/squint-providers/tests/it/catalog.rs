//! Task 2.1: the shipped catalog is complete and sourced, covers the owner's model subset, prices
//! the ledger's requests, and rejects settings a model does not support.
#![allow(clippy::unwrap_used)]

use squint_core::config::Config;
use squint_core::ledger::{Pricing, RequestShape};
use squint_core::provider::Usage;
use squint_providers::catalog::{CATALOG_VERSION, Catalog, TokenEstimate};

fn config(provider: &str, extra: &str) -> Config {
    let text = format!(
        "[provider]\n{provider}\n\n[privacy]\ncloud_calls = true\naccount = \"acct\"\n\n[output]\ndir = \"out\"\n{extra}"
    );
    Config::parse(&text, &[], &|_| None).unwrap()
}

#[test]
fn catalog_shipped_parses_and_every_entry_is_complete() {
    let c = Catalog::shipped().unwrap();
    assert_eq!(c.catalog_version, CATALOG_VERSION);
    assert!(c.models.len() >= 7);
    for (id, m) in &c.models {
        assert!(m.sources.len() >= 2, "{id}: sources");
        assert_eq!(m.verified, "2026-10-26", "{id}: verified today");
        assert!(!m.capabilities.final_turn.is_empty(), "{id}");
        assert!(
            !m.controls.is_empty(),
            "{id}: every model exposes a control"
        );
        assert!(m.images.tokens.max_tokens_per_image() > 0, "{id}");
        assert!(
            c.profiles.values().any(|p| p.adapter == m.adapter),
            "{id}: a shipped profile speaks its adapter"
        );
    }
}

#[test]
fn catalog_covers_the_owner_subset() {
    let c = Catalog::shipped().unwrap();
    for id in [
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
        "gemini-4-argon",
        "gemini-3.8-flash",
        "gpt-6.1-terra",
        "gpt-6.1-sol",
    ] {
        assert!(c.model(id).is_some(), "{id}");
    }
    // Per the providers' documentation on the verified date.
    let opus = c.model("claude-opus-5-5").unwrap();
    assert!(!opus.capabilities.forced_tool_choice && !opus.capabilities.sampling);
    assert_eq!(opus.capabilities.final_turn, ["refuse", "tool_removal"]);
    let haiku = c.model("claude-haiku-5-5").unwrap();
    assert!(haiku.capabilities.forced_tool_choice);
    assert!(haiku.prices.long_prompt.is_some());
    let argon = c.model("gemini-4-argon").unwrap();
    assert!(
        argon
            .unverified
            .iter()
            .any(|u| u == "capabilities.function_call_ids")
    );
    assert!(matches!(
        argon.images.tokens,
        TokenEstimate::Tile {
            tokens_per_tile: 258,
            ..
        }
    ));
}

#[test]
fn catalog_prices_bound_requests_and_cost_usage() {
    let c = Catalog::shipped().unwrap();
    let opus = c.model("claude-opus-5-5").unwrap();
    let shape = RequestShape {
        input_tokens: 1_000_000,
        output_tokens: 100_000,
        images: 2,
    };
    let bound = opus.bound(&shape).unwrap();
    // 1M input at $4 plus two images at 4,784 tokens each, plus 100k output at $20.
    let expected = (1_000_000.0 + 2.0 * 4784.0) * 4.0 / 1e6 + 100_000.0 * 20.0 / 1e6;
    assert!((bound - expected).abs() < 1e-9, "{bound} vs {expected}");
    let cost = opus.cost(&Usage {
        input_tokens: 10_000,
        output_tokens: 1_000,
        ..Default::default()
    });
    assert!((cost - (0.04 + 0.02)).abs() < 1e-12);
    // Haiku's long-prompt tier applies above 100k prompt tokens.
    let haiku = c.model("claude-haiku-5-5").unwrap();
    let short = haiku.cost(&Usage {
        input_tokens: 100_000,
        output_tokens: 0,
        ..Default::default()
    });
    let long = haiku.cost(&Usage {
        input_tokens: 100_001,
        output_tokens: 0,
        ..Default::default()
    });
    assert!((short - 0.01).abs() < 1e-12);
    assert!(long > 0.05 - 1e-9, "{long}");
    // Image token estimates follow the documented rules.
    assert_eq!(opus.images.tokens.tokens_for(1000, 1000), 1296);
    assert_eq!(argon_tokens(&c, 200, 200), 258);
    assert_eq!(argon_tokens(&c, 1536, 1536), 4 * 258);
}

fn argon_tokens(c: &Catalog, w: u32, h: u32) -> u64 {
    c.model("gemini-4-argon")
        .unwrap()
        .images
        .tokens
        .tokens_for(w, h)
}

#[test]
fn catalog_rejects_unsupported_settings() {
    let c = Catalog::shipped().unwrap();
    let ok = config(
        "profile = \"anthropic\"\nmodel = \"claude-opus-5-5\"\ncontrols = { effort = \"high\" }",
        "",
    );
    c.check_config(&ok, false).unwrap();
    let cases: Vec<(&str, &str, &str)> = vec![
        (
            "unknown model",
            "profile = \"anthropic\"\nmodel = \"claude-opus-9\"",
            "not in the catalog",
        ),
        (
            "unknown profile",
            "profile = \"azure\"\nmodel = \"claude-opus-5-5\"",
            "profile \"azure\" is not in the catalog",
        ),
        (
            "adapter mismatch",
            "profile = \"anthropic\"\nmodel = \"gemini-4-argon\"",
            "speaks anthropic-messages but model",
        ),
        (
            "unknown control",
            "profile = \"anthropic\"\nmodel = \"claude-opus-5-5\"\ncontrols = { temperature = \"0\" }",
            "has no control \"temperature\"",
        ),
        (
            "bad control value",
            "profile = \"anthropic\"\nmodel = \"claude-opus-5-5\"\ncontrols = { effort = \"ultra\" }",
            "not supported by \"claude-opus-5-5\"",
        ),
        (
            "non-string control",
            "profile = \"google-ai-studio\"\nmodel = \"gemini-4-argon\"\ncontrols = { thinking_level = 2 }",
            "must be a string",
        ),
        (
            "output over the maximum",
            "profile = \"google-ai-studio\"\nmodel = \"gemini-3.8-flash\"\nmax_output_tokens = 70000",
            "exceeds \"gemini-3.8-flash\"'s maximum",
        ),
    ];
    for (what, provider, message) in cases {
        let err = c.check_config(&config(provider, ""), false).unwrap_err();
        assert_eq!(err.class(), "resolve", "{what}");
        assert!(err.to_string().contains(message), "{what}: {err}");
    }
    let too_large = config(
        "profile = \"openai\"\nmodel = \"gpt-6.1-sol\"",
        "\n[images]\nmax_long_edge = 4096\n",
    );
    let err = c.check_config(&too_large, false).unwrap_err();
    assert!(err.to_string().contains("may only lower"), "{err}");
    let lower = config(
        "profile = \"openai\"\nmodel = \"gpt-6.1-sol\"",
        "\n[images]\nmax_long_edge = 1024\n",
    );
    c.check_config(&lower, false).unwrap();
    let repair = config(
        "profile = \"anthropic\"\nmodel = \"claude-opus-5-5\"",
        "\n[budget]\nrepair_output_tokens = 200000\n",
    );
    assert!(
        c.check_config(&repair, false)
            .unwrap_err()
            .to_string()
            .contains("repair_output_tokens")
    );
}

#[test]
fn catalog_admits_the_scripted_pair_only_when_allowed() {
    let c = Catalog::shipped().unwrap();
    let fake = config("profile = \"fake\"\nmodel = \"fake-model\"", "");
    c.check_config(&fake, true).unwrap();
    let err = c.check_config(&fake, false).unwrap_err();
    assert!(err.to_string().contains("test double"), "{err}");
    let half = config("profile = \"fake\"\nmodel = \"claude-opus-5-5\"", "");
    assert!(
        c.check_config(&half, true).is_err(),
        "the pair must be whole"
    );
}

#[test]
fn catalog_self_test_catches_an_incomplete_entry() {
    let mut text = squint_providers::catalog::SHIPPED.to_string();
    text = text.replacen("verified = \"2026-10-26\"", "verified = \"soon\"", 1);
    let err = Catalog::parse(&text).unwrap_err();
    assert!(err.to_string().contains("verified must be"), "{err}");
    let text = squint_providers::catalog::SHIPPED.replacen(
        "final_turn = [\"refuse\", \"tool_removal\"]",
        "final_turn = [\"refuse\", \"forced\"]",
        1,
    );
    let err = Catalog::parse(&text).unwrap_err();
    assert!(
        err.to_string().contains("forced_tool_choice is false"),
        "{err}"
    );
}

/// Cache reads are charged at the cached rate, cache writes at the write rate (the input rate
/// when the catalog states none), and the long-prompt tier is chosen from the whole prompt.
#[test]
fn cost_prices_cache_reads_and_writes_and_tiers_on_the_whole_prompt() {
    let c = Catalog::shipped().unwrap();
    let haiku = c.model("claude-haiku-5-5").unwrap();
    // 10k uncached at $0.10, 1M cache reads at $0.01, 1M cache writes at $0.125: below the tier.
    let cached = haiku.cost(&Usage {
        input_tokens: 10_000,
        output_tokens: 0,
        cache_read_tokens: 1_000_000,
        cache_write_tokens: 1_000_000,
    });
    let uncached = haiku.cost(&Usage {
        input_tokens: 10_000,
        output_tokens: 0,
        ..Default::default()
    });
    assert!((uncached - 0.001).abs() < 1e-12, "{uncached}");
    // The prompt is 2,010,000 tokens, over the 100k threshold: long-tier prices apply
    // ($0.50 input, $0.05 cached; the write rate has no long tier and stays at $0.125).
    let expected = 10_000.0 * 0.50 / 1e6 + 1_000_000.0 * 0.05 / 1e6 + 1_000_000.0 * 0.125 / 1e6;
    assert!((cached - expected).abs() < 1e-9, "{cached} vs {expected}");
    // A model without a stated write rate charges writes at its input rate.
    let terra = c.model("gpt-6.1-terra").unwrap();
    assert!(terra.prices.cache_write_per_million.is_none());
    let writes = terra.cost(&Usage {
        input_tokens: 0,
        output_tokens: 0,
        cache_read_tokens: 0,
        cache_write_tokens: 1_000_000,
    });
    assert!(
        (writes - terra.prices.input_per_million).abs() < 1e-9,
        "{writes}"
    );
}

/// A model whose final turn may remove tools must accept a system message mid-conversation,
/// since that is where the removal blocks travel.
#[test]
fn catalog_self_test_ties_tool_removal_to_system_messages() {
    let text = squint_providers::catalog::SHIPPED.replacen(
        "system_messages_in_history = true\nfinal_turn = [\"refuse\", \"tool_removal\"]",
        "system_messages_in_history = false\nfinal_turn = [\"refuse\", \"tool_removal\"]",
        1,
    );
    assert_ne!(text, squint_providers::catalog::SHIPPED);
    let err = Catalog::parse(&text).unwrap_err();
    assert!(
        err.to_string()
            .contains("tool_removal but system_messages_in_history is false"),
        "{err}"
    );
}

/// Task 2.1: the self-test names the rule a broken profile or header breaks, one mutation of the
/// shipped catalog per rule.
#[test]
fn catalog_self_test_names_each_broken_profile_rule() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "catalog_version = 1",
            "catalog_version = 7",
            "catalog_version must be",
        ),
        (
            "adapter = \"anthropic-messages\"",
            "adapter = \"anthropic-complete\"",
            "unknown adapter",
        ),
        (
            "scheme = \"api-key-header\", header = \"x-api-key\"",
            "scheme = \"basic\", header = \"x-api-key\"",
            "unknown auth scheme",
        ),
        (
            "account = \"anthropic\"",
            "account = \"anthropic:org\"",
            "account must be a plain prefix",
        ),
        (
            "base_url = \"https://api.anthropic.com\"",
            "base_url = \"http://api.anthropic.com\"",
            "base_url must be https",
        ),
    ];
    for (from, to, message) in cases {
        let text = squint_providers::catalog::SHIPPED.replacen(from, to, 1);
        assert_ne!(
            text,
            squint_providers::catalog::SHIPPED,
            "{from} not in the catalog"
        );
        let err = Catalog::parse(&text).unwrap_err();
        assert_eq!(err.class(), "resolve", "{err}");
        assert!(err.to_string().contains(message), "{from} -> {to}: {err}");
    }
}

/// Task 2.9: every shipped profile has its data-leaving row in docs/provider-data.md, and every
/// shipped model is named there, so a new catalog entry cannot ship without its disclosure.
#[test]
fn provider_data_documents_every_shipped_profile_and_model() {
    let doc = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/provider-data.md"),
    )
    .unwrap();
    let c = Catalog::shipped().unwrap();
    for id in c.profiles.keys() {
        assert!(
            doc.contains(&format!("\n| `{id}` |")),
            "docs/provider-data.md has no row for profile {id}"
        );
    }
    for id in c.models.keys() {
        assert!(
            doc.contains(&format!("`{id}`")),
            "docs/provider-data.md does not name model {id}"
        );
    }
}
