//! Task 7.7a: a contract call's arguments become a squint job (table-manners 10.1): images by
//! absolute path or inline bytes, each with an optional `expect`; references as a non-empty list
//! (since task 9.8, the `many` slot), in order; the brief and requirements; anything else refused.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use serde_json::json;
use squint_core::Error;
use squint_core::contract::job_from_call;
use squint_core::job::{ImageSource, Projection, Role};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn path(name: &str) -> String {
    root()
        .join("fixtures/images")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn args(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().unwrap().clone()
}

#[test]
fn images_by_path_and_inline_become_the_jobs_images() {
    let bytes = std::fs::read(root().join("fixtures/images/source-64x48.png")).unwrap();
    let job = job_from_call(
        "source-fidelity",
        &args(json!({
            "candidate": { "path": path("candidate-64x48.png") },
            "source": { "inline": { "media_type": "image/png", "data": squint_core::image::base64_encode(&bytes) } },
            "references": [ { "path": path("source-64x48.png") }, { "path": path("candidate-64x48.png") } ],
            "brief": "Keep the boat.",
        })),
    )
    .unwrap();
    assert_eq!(job.use_case, "source-fidelity");
    assert_eq!(job.brief.as_deref(), Some("Keep the boat."));
    let ids: Vec<(&str, Role)> = job.images.iter().map(|i| (i.id.as_str(), i.role)).collect();
    assert_eq!(
        ids,
        [
            ("candidate", Role::Candidate),
            ("source", Role::Source),
            ("ref:1", Role::Reference),
            ("ref:2", Role::Reference)
        ]
    );
    assert!(matches!(job.images[0].source, ImageSource::Path { .. }));
    assert!(matches!(job.images[1].source, ImageSource::Base64 { .. }));
}

#[test]
fn an_expect_is_kept_and_names_the_projection() {
    let bytes = std::fs::read(root().join("fixtures/images/candidate-64x48.png")).unwrap();
    let job = job_from_call(
        "reality-checks",
        &args(json!({
            "candidate": {
                "path": path("candidate-64x48.png"),
                "expect": {
                    "file_sha256": squint_core::image::sha256_hex(&bytes),
                    "width": 64, "height": 48,
                    "projection": "equirectangular", "topology": "wrap"
                }
            }
        })),
    )
    .unwrap();
    assert!(job.images[0].expect.is_some());
    assert_eq!(job.images[0].projection, Some(Projection::Equirectangular));
}

#[test]
fn a_relative_path_is_input() {
    let err = job_from_call(
        "reality-checks",
        &args(json!({ "candidate": { "path": "fixtures/images/candidate-64x48.png" } })),
    )
    .unwrap_err();
    assert!(
        matches!(&err, Error::Input(m) if m.contains("absolute")),
        "{err}"
    );
}

#[test]
fn unknown_or_malformed_arguments_are_usage_errors() {
    for bad in [
        json!({ "candidate": { "path": path("candidate-64x48.png") }, "colour": "red" }),
        json!({ "candidate": { "path": path("candidate-64x48.png"), "inline": { "media_type": "image/png", "data": "" } } }),
        json!({ "candidate": "candidate.png" }),
        json!({ "brief": "no candidate" }),
        json!({ "candidate": { "path": path("candidate-64x48.png") }, "references": { "path": "x" } }),
        // A `many` slot takes a non-empty list (table-manners 10.1).
        json!({ "candidate": { "path": path("candidate-64x48.png") }, "references": [] }),
    ] {
        let err = job_from_call("reality-checks", &args(bad.clone())).unwrap_err();
        assert!(matches!(err, Error::Job(_)), "{bad}: {err}");
    }
}

#[test]
fn inline_bytes_that_are_not_base64_are_input() {
    let err = job_from_call(
        "reality-checks",
        &args(json!({ "candidate": { "inline": { "media_type": "image/png", "data": "not base64!" } } })),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Input(_)), "{err}");
}

/// A call's `profile` and `tools` change only that call's copy of the configuration: the profile
/// replaces the configured one (or creates `[rules]` for the use case), `none` asks for the base
/// rules alone, and a tool set narrows `[tools] enabled` to its subset. Values the base or the
/// configuration do not offer are usage errors naming what they do offer.
#[test]
fn call_choices_apply_to_one_call() {
    use squint_core::config::Config;
    use squint_core::contract::CallChoices;
    let text = r#"
[provider]
profile = "anthropic"
model = "claude-haiku-5-5"
[tools]
enabled = ["view", "align", "change_map", "regions"]
order = ["align", "regions", "view", "change_map"]
[privacy]
cloud_calls = true
account = "anthropic:test"
[output]
dir = "./out"
"#;
    let base = Config::parse(text, &[], &|_| None).unwrap();
    let choose = |v: serde_json::Value| CallChoices::from_call(&args(v)).unwrap();

    // Nothing chosen: the configuration as it is.
    let mut config = base.clone();
    let none = choose(json!({}));
    assert!(none.is_empty());
    none.apply("source-fidelity", &mut config).unwrap();
    assert!(config.rules.is_none());
    assert_eq!(config.tools.enabled, base.tools.enabled);

    // A profile with no `[rules]`: created for the use case.
    let mut config = base.clone();
    choose(json!({ "profile": "architecture" }))
        .apply("source-fidelity", &mut config)
        .unwrap();
    let rules = config.rules.as_ref().unwrap();
    assert_eq!(rules.base, "source-fidelity");
    assert_eq!(rules.profile.as_deref(), Some("architecture"));

    // `none` clears a configured profile.
    choose(json!({ "profile": "none" }))
        .apply("source-fidelity", &mut config)
        .unwrap();
    assert_eq!(config.rules.as_ref().unwrap().profile, None);

    // A profile of another base, or an unknown one, is refused.
    let err = choose(json!({ "profile": "architecture" }))
        .apply("reality-checks", &mut base.clone())
        .unwrap_err();
    assert!(matches!(err, Error::Job(_)), "{err}");
    assert!(err.to_string().contains("[\"none\"]"), "{err}");

    // Tool sets narrow the enabled tools, keeping their order.
    for (set, expected) in [
        ("none", vec![]),
        ("view_align", vec!["view", "align"]),
        ("all", vec!["view", "align", "change_map", "regions"]),
    ] {
        let mut config = base.clone();
        choose(json!({ "tools": set }))
            .apply("source-fidelity", &mut config)
            .unwrap();
        assert_eq!(config.tools.enabled, expected, "{set}");
        assert!(
            config
                .tools
                .order
                .iter()
                .all(|t| expected.contains(&t.as_str()))
        );
    }
    let err = choose(json!({ "tools": "regions_only" }))
        .apply("source-fidelity", &mut base.clone())
        .unwrap_err();
    assert!(err.to_string().contains("view_align"), "{err}");

    // A configuration with no tools offers no tool sets.
    let mut bare = base.clone();
    bare.tools.enabled.clear();
    let err = choose(json!({ "tools": "none" }))
        .apply("source-fidelity", &mut bare)
        .unwrap_err();
    assert!(err.to_string().contains("enables no tools"), "{err}");

    // The wrong type is refused before anything is applied.
    assert!(CallChoices::from_call(&args(json!({ "tools": ["view"] }))).is_err());

    // Both are call arguments a job accepts beside its images.
    let job = job_from_call(
        "source-fidelity",
        &args(json!({
            "candidate": { "path": path("candidate-64x48.png") },
            "source": { "path": path("candidate-64x48.png") },
            "profile": "architecture",
            "tools": "none"
        })),
    );
    assert!(job.is_ok(), "{job:?}");
}
