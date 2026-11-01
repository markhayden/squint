//! Locates the fake MCP server binary (`squint-testkit-mcp`): the prebuilt one named by
//! `SQUINT_TESTKIT_MCP` (as `just test` builds it once, before tests that run one process each),
//! else built with Cargo on first use, its absolute path read from Cargo's JSON output, so no test
//! depends on `PATH` or a previous build.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

static PATH: OnceLock<PathBuf> = OnceLock::new();

/// The absolute path of the fake server executable.
pub fn path() -> PathBuf {
    PATH.get_or_init(build).clone()
}

fn build() -> PathBuf {
    if let Some(prebuilt) = std::env::var_os("SQUINT_TESTKIT_MCP").map(PathBuf::from)
        && prebuilt.is_file()
    {
        return prebuilt;
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = Command::new(cargo)
        .args([
            "build",
            "--quiet",
            "--manifest-path",
            manifest.to_str().expect("utf-8 path"),
            "--bin",
            "squint-testkit-mcp",
            "--message-format=json-render-diagnostics",
        ])
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "building squint-testkit-mcp failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if v["reason"] == "compiler-artifact"
            && v["target"]["name"] == "squint-testkit-mcp"
            && let Some(exe) = v["executable"].as_str()
        {
            return PathBuf::from(exe);
        }
    }
    panic!("cargo's output named no executable for squint-testkit-mcp");
}
