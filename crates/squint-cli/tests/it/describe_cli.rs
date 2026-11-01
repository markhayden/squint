//! Task 7.5 CLI scenario: `squint describe --json` prints the contract's description, naming the
//! credential variable of the configured profile and never a value; since task 9.1, in each
//! action's readiness, `present` or `missing` (table-manners 3.1).
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn describe_prints_the_description_and_no_secret() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "describe",
            "--json",
            "--config",
            "examples/source-fidelity.toml",
        ])
        .env("ANTHROPIC_API_KEY", "sk-describe-secret-value")
        .env("SQUINT_ANTHROPIC_ACCOUNT", "org-describe-secret-account")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&o.stdout);
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!stdout.contains("sk-describe-secret-value"));
    assert!(!stdout.contains("org-describe-secret-account"));
    let doc: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(doc["profiles"], serde_json::json!(["images"]));
    let sf = doc["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "source_fidelity")
        .unwrap();
    assert_eq!(
        sf["readiness"],
        serde_json::json!({ "state": "ready", "requirements": [{
            "kind": "credential",
            "name": "ANTHROPIC_API_KEY",
            "state": "present",
            "setup": "Set ANTHROPIC_API_KEY in the environment squint is started with."
        }]})
    );
    assert!(!stdout.contains("needs_env"));
    // A real provider may spend (squint issue #2).
    assert_eq!(sf["cost"]["may_spend"], true);
}

/// With the credential unset (or blank), every paid action needs setup and names the variable.
#[test]
fn describe_names_a_missing_credential() {
    for unset in [None, Some("  ")] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_squint"));
        command.current_dir(repo_root()).args([
            "describe",
            "--json",
            "--config",
            "examples/source-fidelity.toml",
        ]);
        match unset {
            None => command.env_remove("ANTHROPIC_API_KEY"),
            Some(blank) => command.env("ANTHROPIC_API_KEY", blank),
        };
        let o = command
            .env("SQUINT_ANTHROPIC_ACCOUNT", "org-describe")
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(0));
        let doc: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
        for action in doc["actions"].as_array().unwrap() {
            assert_eq!(action["readiness"]["state"], "needs_setup", "{unset:?}");
            assert_eq!(
                action["readiness"]["requirements"][0]["state"], "missing",
                "{unset:?}"
            );
            assert_eq!(
                action["readiness"]["requirements"][0]["name"],
                "ANTHROPIC_API_KEY"
            );
        }
    }
}

/// The fake profile can never reach a paid provider: its checks declare no spend and no
/// credential (squint issue #2), so a client that refuses paid work can run them.
#[test]
fn the_fake_profile_declares_free_checks() {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "describe",
            "--json",
            "--config",
            "fixtures/config/minimal.toml",
        ])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0));
    let doc: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    for action in doc["actions"].as_array().unwrap() {
        assert_eq!(action["cost"]["may_spend"], false, "{}", action["name"]);
        assert_eq!(
            action["readiness"],
            serde_json::json!({ "state": "ready", "requirements": [] }),
            "{}",
            action["name"]
        );
    }
}

/// A scratch directory: the working directory, `$XDG_CONFIG_HOME` and `$HOME` of a describe that
/// must not find the developer's own configuration.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-describe-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("xdg")).unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();
    std::fs::create_dir_all(dir.join("work")).unwrap();
    dir
}

/// `squint describe --json` run in `scratch/work`, with the user's config directory in `scratch`.
fn describe_in(scratch: &Path, extra: &[&str]) -> (Option<i32>, serde_json::Value) {
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(scratch.join("work"))
        .args(["describe", "--json"])
        .args(extra)
        .env("XDG_CONFIG_HOME", scratch.join("xdg"))
        .env("HOME", scratch.join("home"))
        .env_remove("SQUINT_ANTHROPIC_ACCOUNT")
        .output()
        .unwrap();
    let doc = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&o.stderr)));
    valid(&doc);
    (o.status.code(), doc)
}

/// The document against the contract's description schema.
fn valid(doc: &serde_json::Value) {
    struct Embedded;
    impl jsonschema::Retrieve for Embedded {
        fn retrieve(
            &self,
            uri: &jsonschema::Uri<String>,
        ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
            let (_, text) = table_manners::SCHEMAS
                .iter()
                .find(|(id, _)| *id == uri.as_str())
                .ok_or("unknown schema")?;
            Ok(serde_json::from_str(text)?)
        }
    }
    let (_, text) = table_manners::SCHEMAS
        .iter()
        .find(|(id, _)| *id == "urn:table-manners:core:description")
        .unwrap();
    let schema: serde_json::Value = serde_json::from_str(text).unwrap();
    let validator = jsonschema::options()
        .with_retriever(Embedded)
        .build(&schema)
        .unwrap();
    let errors: Vec<String> = validator.iter_errors(doc).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{errors:?}");
}

/// Every action needs setup for the configuration alone, in `state`, naming `path`.
fn needs_the_config(doc: &serde_json::Value, state: &str, path: &Path) {
    let actions = doc["actions"].as_array().unwrap();
    assert_eq!(actions.len(), 5, "every shipped use case");
    for action in actions {
        assert_eq!(action["readiness"]["state"], "needs_setup", "{action}");
        let requirements = action["readiness"]["requirements"].as_array().unwrap();
        assert_eq!(requirements.len(), 1, "{action}");
        assert_eq!(requirements[0]["kind"], "config");
        assert_eq!(requirements[0]["state"], state);
        assert_eq!(requirements[0]["name"], path.to_str().unwrap());
    }
}

/// No configuration anywhere on the lookup is not an error (table-manners 3.1, task 9.2): exit 0,
/// a valid document, every action needing the configuration the lookup would use.
#[test]
fn describe_works_with_no_configuration() {
    let dir = scratch("none");
    let (code, doc) = describe_in(&dir, &[]);
    assert_eq!(code, Some(0));
    needs_the_config(&doc, "missing", &dir.join("xdg/squint/squint.toml"));
    assert!(
        doc["actions"][0]["readiness"]["requirements"][0]["setup"]
            .as_str()
            .unwrap()
            .contains("--config")
    );
}

/// `--config` naming a file that does not exist: the same, naming that path.
#[test]
fn describe_names_a_missing_given_config() {
    let dir = scratch("given");
    let given = dir.join("nowhere/squint.toml");
    let (code, doc) = describe_in(&dir, &["--config", given.to_str().unwrap()]);
    assert_eq!(code, Some(0));
    needs_the_config(&doc, "missing", &given);
}

/// A configuration that cannot be read, one that does not parse, and one whose `${VAR}` is
/// unset are each `invalid`, and still exit 0.
#[test]
fn describe_reports_an_unloadable_config_as_invalid() {
    let dir = scratch("invalid");
    let unreadable = dir.join("work/squint.toml");
    std::fs::create_dir_all(&unreadable).unwrap();
    let (code, doc) = describe_in(&dir, &[]);
    assert_eq!(code, Some(0));
    needs_the_config(
        &doc,
        "invalid",
        &std::fs::canonicalize(&unreadable).unwrap(),
    );

    let broken = dir.join("broken.toml");
    std::fs::write(&broken, "[provider\n").unwrap();
    let (code, doc) = describe_in(&dir, &["--config", broken.to_str().unwrap()]);
    assert_eq!(code, Some(0));
    needs_the_config(&doc, "invalid", &broken);

    let example = repo_root().join("examples/source-fidelity.toml");
    let (code, doc) = describe_in(&dir, &["--config", example.to_str().unwrap()]);
    assert_eq!(code, Some(0));
    needs_the_config(&doc, "invalid", &example);
    let setup = doc["actions"][0]["readiness"]["requirements"][0]["setup"]
        .as_str()
        .unwrap();
    assert!(setup.contains("SQUINT_ANTHROPIC_ACCOUNT"), "{setup}");
}

/// `./squint.toml` is found from the working directory, and `--config` wins over it.
#[test]
fn the_working_directory_config_is_found_and_config_wins() {
    let dir = scratch("cwd");
    std::fs::copy(
        repo_root().join("fixtures/config/minimal.toml"),
        dir.join("work/squint.toml"),
    )
    .unwrap();
    let (code, doc) = describe_in(&dir, &[]);
    assert_eq!(code, Some(0));
    // The fake profile: free and ready.
    for action in doc["actions"].as_array().unwrap() {
        assert_eq!(action["cost"]["may_spend"], false);
        assert_eq!(action["readiness"]["state"], "ready");
    }
    let example = repo_root().join("examples/source-fidelity.toml");
    let broken = dir.join("broken.toml");
    std::fs::write(&broken, "[provider\n").unwrap();
    let (_, doc) = describe_in(&dir, &["--config", broken.to_str().unwrap()]);
    needs_the_config(&doc, "invalid", &broken);
    let (_, doc) = describe_in(&dir, &["--config", example.to_str().unwrap()]);
    assert_eq!(doc["actions"][0]["cost"]["may_spend"], true, "{doc}");
}
