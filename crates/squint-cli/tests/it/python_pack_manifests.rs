//! Task 5.4a: every manifest the Python pack lists passes the three adapters' wire
//! transformations beside `submit_verdict`, so a run with the pack enabled can be resolved for
//! any shipped profile, and every image and frame argument names both forms it takes (F2). Needs `uv` and a synced `tools/python` environment, so it runs with
//! `--ignored`; CI's python job runs it.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::time::Duration;

use squint_core::provider::ToolDefinition;
use squint_core::submission;
use squint_providers::catalog::Catalog;
use squint_providers::wire::{Adapter, check_tool_set};
use squint_tools::mcp::{Pack, Spawn, Timeouts};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
#[ignore = "needs uv, a synced tools/python environment and the fetched segmenter (just python-check, squint-tools fetch)"]
fn every_pack_manifest_passes_every_adapters_wire_transformation() {
    let pack = Pack::connect(
        "python",
        Spawn {
            command: PathBuf::from("uv"),
            args: [
                "run",
                "--frozen",
                "--offline",
                "--no-sync",
                "squint-tools",
                "serve",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            declared_env: vec![],
            cwd: Some(root().join("tools/python")),
            extra_env: vec![],
        },
        Timeouts {
            startup: Duration::from_secs(60),
            call: Duration::from_secs(30),
            shutdown_grace: Duration::from_millis(500),
        },
        std::env::vars(),
    )
    .unwrap();
    let names: Vec<&str> = pack.manifests().iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"regions"), "{names:?}");
    let regions = pack
        .manifests()
        .iter()
        .find(|m| m.name == "regions")
        .unwrap();
    assert_eq!(regions.requires, ["align"]);
    assert_eq!(regions.frame_args, ["frame", "against"]);
    assert_eq!(regions.accepts_hints, ["regions"]);
    // Every image and frame argument says it takes an image id or a raw frame id (F2).
    let gaps: Vec<String> = pack
        .manifests()
        .iter()
        .flat_map(squint_testkit::tool::image_argument_gaps)
        .collect();
    assert!(gaps.is_empty(), "{gaps:#?}");

    let mut defs: Vec<ToolDefinition> = pack
        .manifests()
        .iter()
        .map(|m| m.tool_definition())
        .collect();
    defs.push(ToolDefinition {
        name: "submit_verdict".into(),
        description: "the verdict".into(),
        input_schema: submission::canonical_schema(),
    });
    let catalog = Catalog::shipped().unwrap();
    for (adapter, model) in [
        (Adapter::AnthropicMessages, "claude-opus-5-5"),
        (Adapter::GeminiGenerateContent, "gemini-4-argon"),
        (Adapter::OpenaiResponses, "gpt-6.1-sol"),
    ] {
        let caps = &catalog.model(model).unwrap().capabilities;
        let wired =
            check_tool_set(adapter, caps, &defs).unwrap_or_else(|e| panic!("{adapter:?}: {e}"));
        assert_eq!(wired.len(), defs.len());
    }
    pack.shutdown();
}

#[test]
#[ignore = "needs a synced tools/python environment (just python-check)"]
fn a_check_using_regions_without_the_segmenter_ends_at_resolve_naming_the_file() {
    let dir = std::env::temp_dir().join(format!("squint-pack-no-models-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cache_home = dir.join("cache");
    std::fs::create_dir_all(&cache_home).unwrap();
    let out = dir.join("out");
    let executable = root().join("tools/python/.venv/bin/squint-tools");
    let config = format!(
        "[provider]\nprofile = \"fake\"\nmodel = \"fake-model\"\n\n[tools]\nenabled = [\"align\", \"regions\"]\n\n[tools.packs.python]\ncommand = {:?}\nargs = [\"serve\"]\nenv = [\"XDG_CACHE_HOME\"]\n\n[privacy]\ncloud_calls = true\naccount = \"fake:test\"\n\n[output]\ndir = {:?}\n",
        executable.display().to_string(),
        out.display().to_string()
    );
    let config_path = dir.join("config.toml");
    std::fs::write(&config_path, config).unwrap();
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(root())
        .env("XDG_CACHE_HOME", &cache_home)
        .args([
            "check",
            "--config",
            config_path.to_str().unwrap(),
            "--job",
            "fixtures/jobs/pair.toml",
            "--dry-run",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(64), "{stderr}");
    let artifact = cache_home.join("squint/models/segmenter-v1/eomt-large-ade20k-512.onnx");
    assert!(
        stderr.contains("regions and classes need the segmenter artifact")
            && stderr.contains(&format!("{} is missing", artifact.display()))
            && stderr.contains("`squint-tools fetch`"),
        "{stderr}"
    );
    assert!(!out.exists(), "nothing was written: the run never started");
}
