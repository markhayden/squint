//! Task 1.9 CLI scenario: `check` with `spend_cap_usd = 0.01` exits 3 before any request, the
//! per-call ledger's refusal `cap` since task 9.5; the report shows the envelope.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
fn a_one_cent_cap_refuses_the_envelope_before_any_request() {
    let out = std::env::temp_dir().join(format!("squint-budget-cli-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/minimal.toml",
            "--set",
            "budget.spend_cap_usd=0.01",
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            "fixtures/scripts/held.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("cap") && err.contains("envelope"), "{err}");
    let dir = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("report.json")).unwrap()).unwrap();
    assert_eq!(r["runs"][0]["error"]["class"], "cap");
    assert!(r["runs"][0]["turns"].as_array().unwrap().is_empty());
    assert!(r["cost"]["envelope"]["usd"].as_f64().unwrap() > 0.01);
    assert_eq!(r["cost"]["cap_usd"], 0.01);
}

fn check(out: &Path, extra: &[&str]) -> (Option<i32>, String) {
    let mut args = vec![
        "check",
        "--config",
        "fixtures/config/minimal.toml",
        "--job",
        "fixtures/jobs/pair.toml",
        "--out",
        out.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args(&args)
        .output()
        .unwrap();
    (
        o.status.code(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

/// Task 11.4: a cap refusal explains its envelope (the requests by kind, the retry reserve, the
/// runs), says it is a worst case and not an expected cost, names the settings that move it with
/// their values, and shows real spend only: none without an earlier decision, the earlier
/// decision's settled spend with one.
#[test]
fn a_cap_refusal_explains_the_envelope_and_shows_only_real_spend() {
    let out = std::env::temp_dir().join(format!("squint-budget-why-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();
    let refuse = [
        "--provider-script",
        "fixtures/scripts/held.json",
        "--max-spend-usd",
        "0.05",
    ];
    let (code, err) = check(&out, &refuse);
    assert_eq!(code, Some(3), "{err}");
    for expected in [
        "it is a worst-case bound, not an expected cost",
        "The check was refused before any request: nothing was sent.",
        "4 × tool_turn  up to 16000 output tokens each",
        "1 × final_turn up to 16000 output tokens,",
        "1 × repair     up to 32000 output tokens,",
        "retry reserve: 3 retries × the largest request",
        "a run × 1 decision run = $",
        "prices: fake-model is not in the catalog",
        "--max-spend-usd (up to the configured $2.00)",
        "budget.tool_turns=N (now 4)",
        "provider.max_output_tokens=N (now 16000)",
        "budget.repairs=N (now 1)",
        "budget.retries=N (now 3)",
    ] {
        assert!(err.contains(expected), "{expected:?} missing from:\n{err}");
    }
    assert!(
        !err.contains("actually spent"),
        "no earlier decision, no figure: {err}"
    );

    // One completed check (it reports 500k input and 100k output tokens: $0.09 at the scripted
    // prices), then the same refusal shows what it really spent.
    let (code, err) = check(
        &out,
        &["--provider-script", "fixtures/scripts/held-costly.json"],
    );
    assert_eq!(code, Some(0), "{err}");
    let (code, err) = check(&out, &refuse);
    assert_eq!(code, Some(3), "{err}");
    let line = err
        .lines()
        .find(|l| {
            l.contains("actually spent by recent completed source-fidelity checks on fake-model")
        })
        .unwrap_or_else(|| panic!("{err}"));
    assert!(line.ends_with("(newest first): $0.0900"), "{line}");
}

/// The tier line on a catalog model: Claude Haiku 5.5's planned repair passes its 100k-token
/// long-prompt threshold. Refused before any request, so the replay is never read.
#[test]
fn a_cap_refusal_names_the_long_prompt_tier_and_why() {
    let out = std::env::temp_dir().join(format!("squint-budget-tier-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .env("SQUINT_ANTHROPIC_ACCOUNT", "org-tier")
        .env_remove("ANTHROPIC_API_KEY")
        .args([
            "check",
            "--config",
            "examples/reference-consistency.toml",
            "--use-case",
            "reference-consistency",
            "--candidate",
            "fixtures/images/candidate-64x48.png",
            "--ref",
            "dog=fixtures/images/source-64x48.png",
            "--replay",
            "fixtures/adapters/anthropic-messages/first-look-haiku",
            "--max-spend-usd",
            "0.10",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(3), "{err}");
    assert!(
        err.contains("bounded at claude-haiku-5-5's long-prompt tier ($0.5/M input, $2.5/M output tokens, against $0.1/M and $0.5/M below it)")
            && err.contains("pass its 100000-token threshold"),
        "{err}"
    );
}
