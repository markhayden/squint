//! `ToolSession`: tools run with no model through the harness's own call path, and the hints the
//! rules name reach a tool keyed by the hint name its manifest accepts.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use squint_core::config::Config;
use squint_core::job::{ImageSource, ImageSpec, Job, Role};
use squint_core::run::ToolSession;
use squint_core::tool::{Envelope, Manifest, SquintContext, Tool, ToolRegistry};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A tool named `regions` (accepting `hints.regions`) that keeps the context it was handed.
struct HintProbe {
    manifest: Manifest,
    seen: Arc<Mutex<Option<serde_json::Value>>>,
}

impl Tool for HintProbe {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    fn call(&self, _: &serde_json::Value, ctx: &SquintContext) -> squint_core::Result<Envelope> {
        *self.seen.lock().unwrap() = Some(ctx.hints.clone());
        Ok(serde_json::from_value(serde_json::json!({
            "envelope_version": 1,
            "status": "measured_nothing",
            "summary": "probe",
            "measurements": [], "regions": [], "images": [], "transforms": [],
            "provenance": { "tool": "regions", "version": "1.0.0", "duration_ms": 0, "lock_digest": null }
        }))
        .unwrap())
    }
}

fn image(id: &str, role: Role, file: &str) -> ImageSpec {
    ImageSpec {
        id: id.into(),
        role,
        label: None,
        source: ImageSource::Path {
            path: root().join("fixtures/images").join(file),
            sha256: None,
        },
        projection: None,
        expect: None,
    }
}

#[test]
fn a_tool_gets_the_hint_tables_it_accepts_keyed_by_name() {
    let text = std::fs::read_to_string(root().join("fixtures/config/minimal.toml")).unwrap();
    let overrides = vec![
        "rules.base=source-fidelity".to_string(),
        "rules.profile=architecture".to_string(),
    ];
    let config = Config::parse(&text, &overrides, &|_| None).unwrap();
    let manifest = Manifest::from_toml(
        &std::fs::read_to_string(root().join("fixtures/tools/regions.toml"))
            .unwrap()
            .replace("requires = [\"align\"]", "requires = []"),
    )
    .unwrap();
    let seen = Arc::new(Mutex::new(None));
    let tools = ToolRegistry::new(
        vec![Box::new(HintProbe {
            manifest,
            seen: Arc::clone(&seen),
        })],
        &["regions".into()],
        &[],
    )
    .unwrap();
    let job = Job {
        use_case: "source-fidelity".into(),
        brief: None,
        requirements: vec![],
        images: vec![
            image("candidate", Role::Candidate, "candidate-64x48.png"),
            image("source", Role::Source, "source-64x48.png"),
        ],
    };
    let dir = std::env::temp_dir().join(format!("squint-tool-session-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut session = ToolSession::open(&config, &job, &root(), &tools, &dir).unwrap();
    let record = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(session.call(
            "regions",
            serde_json::json!({ "image": "candidate", "against": "raw:source" }),
        ))
        .unwrap();
    assert_eq!(record.rejected, None);
    let hints = seen.lock().unwrap().clone().unwrap();
    // The architecture profile's `[hints.regions]`, under its name, as the pack reads it.
    let preserve = hints["regions"]["preserve"].as_array().unwrap();
    assert!(preserve.iter().any(|c| c == "swimming pool"), "{hints}");
    // A table the tool does not accept is not sent.
    assert!(hints.get("outline").is_none(), "{hints}");
}
