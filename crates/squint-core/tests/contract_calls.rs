//! Task 7.7a: a contract call's arguments become a squint job (table-manners 10.1): images by
//! absolute path or inline bytes, each with an optional `expect`; references as a list; the
//! brief and requirements; anything else refused.
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
