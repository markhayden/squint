//! Task 11.4: `squint init` writes the personal default config where the lookup reads it, with no
//! secret in it, names the variables a run reads, and never overwrites.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// A fresh home with an XDG config directory and an empty working directory.
fn home_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-init-cli-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("work")).unwrap();
    dir
}

fn squint(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(home.join("work"))
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join("xdg"))
        .env("SQUINT_ANTHROPIC_ACCOUNT", "org-init")
        .env_remove("ANTHROPIC_API_KEY")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn init_writes_the_config_the_lookup_finds_and_never_overwrites_it() {
    let home = home_dir("default");
    let o = squint(&home, &["init"]);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(0), "{stderr}");
    let path = home.join("xdg/squint/squint.toml");
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        format!("wrote {}\n", path.display())
    );
    assert!(
        stderr.contains("ANTHROPIC_API_KEY") && stderr.contains("SQUINT_ANTHROPIC_ACCOUNT"),
        "{stderr}"
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("model = \"claude-haiku-5-5\""), "{text}");
    assert!(text.contains("account = \"anthropic:${SQUINT_ANTHROPIC_ACCOUNT}\""));
    assert!(!text.contains("org-init"), "no expanded value is written");

    // `check` finds it with no --config.
    let root = repo_root();
    let image = |name: &str| root.join("fixtures/images").join(name);
    let candidate = image("candidate-64x48.png");
    let source = image("source-64x48.png");
    let o = squint(
        &home,
        &[
            "check",
            "--use-case",
            "source-fidelity",
            "--candidate",
            candidate.to_str().unwrap(),
            "--source",
            source.to_str().unwrap(),
            "--dry-run",
        ],
    );
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("SF1@candidate"));

    let o = squint(&home, &["init", "--provider", "openai"]);
    assert_eq!(o.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&o.stderr).contains("never overwrites"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
}

#[test]
fn init_takes_a_provider_and_model_from_the_catalog() {
    let home = home_dir("openai");
    let o = squint(&home, &["init", "--provider", "openai"]);
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(0), "{stderr}");
    assert!(stderr.contains("OPENAI_API_KEY") && stderr.contains("SQUINT_OPENAI_ACCOUNT"));
    let text = std::fs::read_to_string(home.join("xdg/squint/squint.toml")).unwrap();
    assert!(text.contains("profile = \"openai\"") && text.contains("model = \"gpt-6.1-terra\""));

    let bad = home_dir("bad");
    for args in [
        ["init", "--model", "no-such-model"].as_slice(),
        [
            "init",
            "--provider",
            "openai",
            "--model",
            "claude-haiku-5-5",
        ]
        .as_slice(),
        ["init", "--provider", "nope"].as_slice(),
    ] {
        let o = squint(&bad, args);
        assert_eq!(o.status.code(), Some(64), "{args:?}");
        assert!(!bad.join("xdg/squint/squint.toml").exists(), "{args:?}");
    }
}
