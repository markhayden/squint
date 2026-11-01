//! Tasks 4.3a and 4.3b: the `align` built-in. A synthetic flat pair registers within the ADR 0014
//! tolerances and yields an aligned frame that matches the target; an unrelated pair is
//! `not_applicable` with an `alignment_failure` region; `visual_only` keeps the frame and the
//! transform and withholds the rest; a panorama pair registers by rotation; through a run, a
//! finding on the aligned frame maps back to the candidate.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use image::RgbImage;
use squint_core::config::Config;
use squint_core::frames::Mapping;
use squint_core::job::{Job, Projection};
use squint_core::run;
use squint_core::tool::{ImageHandle, Projected, SquintContext, Status, Tool, ToolRegistry};
use squint_testkit::ScriptedProvider;
use squint_testkit::images::{self, IDENTITY, Scene, SphereScene, similarity};
use squint_tools::builtin;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-align-tool-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn context(dir: &Path, images: &[(&str, &RgbImage, Projection)]) -> SquintContext {
    let mut ctx = SquintContext {
        run_id: "run".into(),
        call_id: "call:1".into(),
        ..Default::default()
    };
    for (id, img, projection) in images {
        let path = dir.join(format!("{id}.png"));
        img.save(&path).unwrap();
        ctx.images.insert(
            (*id).to_string(),
            ImageHandle {
                media_type: "image/png".into(),
                width: img.width(),
                height: img.height(),
                projection: *projection,
                path: Some(path.display().to_string()),
                base64: None,
            },
        );
    }
    ctx
}

fn decode(b64: &str) -> RgbImage {
    let bytes = squint_core::image::base64_decode(b64).unwrap();
    image::load_from_memory(&bytes).unwrap().to_rgb8()
}

fn measurement(e: &squint_core::tool::Envelope, name: &str) -> f64 {
    e.measurements
        .iter()
        .find(|m| m.name == name)
        .unwrap()
        .value
}

#[test]
fn a_flat_pair_registers_with_an_aligned_frame_that_matches_the_target() {
    let dir = tempdir("flat");
    let scene = Scene::random(1, 220, 0.75);
    let source = scene.render(512, 384);
    // The candidate is the scene shifted and turned a little: the truth maps source → candidate.
    let truth = similarity(512, 384, 1.0, 2.0, 6.0, -4.0);
    let candidate = scene.render_warped(512, 384, &truth);
    let ctx = context(
        &dir,
        &[
            ("candidate", &candidate, Projection::Flat),
            ("source", &source, Projection::Flat),
        ],
    );
    let tool = builtin::align::Align::new();
    let e = tool
        .call(
            &serde_json::json!({ "image": "candidate", "against": "source" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::Ok, "{}", e.summary);
    e.check_bounds(&tool.manifest().returns).unwrap();
    assert_eq!(e.transforms.len(), 1);
    let t = &e.transforms[0];
    assert_eq!(
        (t.from.as_str(), t.to.as_str(), t.kind.as_str()),
        ("raw:candidate", "raw:source", "homography")
    );
    assert_eq!(t.params.len(), 9);
    assert!(t.quality["ncc"].as_f64().unwrap() > 0.95, "{}", t.quality);
    // The transform takes candidate coordinates to source coordinates: the inverse of the truth,
    // in normalised units. Check it on the centre and a corner, within a tenth of a pixel.
    let inv = images::invert(&truth);
    let h: [f64; 9] = t.params.clone().try_into().unwrap();
    for (x, y) in [(256.0, 192.0), (40.0, 30.0), (480.0, 350.0)] {
        let (ex, ey) = images::apply(&inv, x, y);
        let (nx, ny) = images::apply(&h, x / 512.0, y / 384.0);
        assert!(
            ((nx * 512.0 - ex).powi(2) + (ny * 384.0 - ey).powi(2)).sqrt() < 0.1,
            "{x},{y}"
        );
    }
    assert!(
        measurement(&e, "rotation_deg").abs() > 1.5 && measurement(&e, "rotation_deg").abs() < 2.5
    );
    assert!(measurement(&e, "ncc") > 0.95 && measurement(&e, "inlier_fraction") > 0.9);
    // The aligned frame: the candidate warped into the source's grid, so it matches the source
    // closely wherever the candidate had pixels to give.
    assert_eq!(e.images.len(), 1);
    let img = &e.images[0];
    assert_eq!(img.frame, "aligned:candidate:t1");
    assert_eq!(img.parent, "raw:source");
    assert_eq!(img.mapping, Mapping::Homography(t.params.clone()));
    assert_eq!((img.width, img.height), (512, 384));
    let warped = decode(img.base64.as_ref().unwrap());
    let mut diff = 0.0f64;
    let mut n = 0usize;
    for (x, y, p) in warped.enumerate_pixels() {
        if x < 20 || y < 20 || x > 490 || y > 360 {
            continue;
        }
        let q = source.get_pixel(x, y);
        diff += (0..3)
            .map(|k| f64::from(p.0[k].abs_diff(q.0[k])))
            .sum::<f64>()
            / 3.0;
        n += 1;
    }
    assert!(
        diff / (n as f64) < 3.0,
        "mean difference {}",
        diff / n as f64
    );
    assert!(e.regions.is_empty());
    assert!(
        e.summary.contains("registered onto source"),
        "{}",
        e.summary
    );
}

#[test]
fn an_unrelated_pair_is_not_applicable_with_an_alignment_failure_region() {
    let dir = tempdir("unrelated");
    let a = Scene::random(10, 220, 0.75).render(384, 288);
    let b = Scene::random(11, 220, 0.75).render(384, 288);
    let ctx = context(
        &dir,
        &[
            ("candidate", &a, Projection::Flat),
            ("source", &b, Projection::Flat),
        ],
    );
    let tool = builtin::align::Align::new();
    let e = tool
        .call(
            &serde_json::json!({ "image": "candidate", "against": "source" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::NotApplicable, "{}", e.summary);
    assert!(e.images.is_empty() && e.transforms.is_empty());
    assert_eq!(e.regions.len(), 1);
    assert_eq!(e.regions[0].location.frame, "raw:candidate");
    assert_eq!(
        e.regions[0].location.kind,
        squint_core::frames::LocationKind::WholeImage
    );
    assert!(e.summary.contains("does not register"), "{}", e.summary);
    // Textureless: not applicable, and no region, since nothing was learnt about the geometry.
    let flat_a = images::low_texture(256, 192, 1, 0.0);
    let flat_b = images::low_texture(256, 192, 2, 0.0);
    let ctx = context(
        &dir,
        &[
            ("candidate", &flat_a, Projection::Flat),
            ("source", &flat_b, Projection::Flat),
        ],
    );
    let e = tool
        .call(
            &serde_json::json!({ "image": "candidate", "against": "source" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::NotApplicable);
    assert!(e.regions.is_empty(), "{}", e.summary);
    assert!(e.summary.contains("too little texture"), "{}", e.summary);
}

/// A numerical failure of the estimator is not evidence about the pair: `not_applicable`, no
/// region, and a summary that says the registration did not converge.
#[test]
fn a_registration_that_does_not_converge_gives_no_evidence() {
    let dir = tempdir("not-converged");
    let scene = Scene::random(4, 220, 0.75);
    let source = scene.render(256, 192);
    let ctx = context(
        &dir,
        &[
            ("candidate", &source, Projection::Flat),
            ("source", &source, Projection::Flat),
        ],
    );
    let tool = builtin::align::Align::with_options(squint_tools::align::Options {
        max_iterations: 0,
        ..squint_tools::align::Options::default()
    });
    let e = tool
        .call(
            &serde_json::json!({ "image": "candidate", "against": "source" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::NotApplicable, "{}", e.summary);
    assert!(e.regions.is_empty(), "{}", e.summary);
    assert!(e.images.is_empty() && e.transforms.is_empty());
    assert!(e.summary.contains("did not converge"), "{}", e.summary);
}

#[test]
fn align_refuses_mixed_projections_and_the_same_image_twice() {
    let dir = tempdir("invalid");
    let flat = Scene::random(3, 60, 0.75).render(128, 96);
    let pano = SphereScene::random(3, 40).render(128, 64, &IDENTITY);
    let ctx = context(
        &dir,
        &[
            ("candidate", &flat, Projection::Flat),
            ("pano", &pano, Projection::Equirectangular),
        ],
    );
    let tool = builtin::align::Align::new();
    let e = tool
        .call(
            &serde_json::json!({ "image": "candidate", "against": "pano" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::InvalidArgs);
    assert!(e.summary.contains("one projection"), "{}", e.summary);
    let e = tool
        .call(
            &serde_json::json!({ "image": "candidate", "against": "candidate" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::InvalidArgs);
    let e = tool
        .call(
            &serde_json::json!({ "image": "candidate", "against": "nope" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::InvalidArgs);
    assert!(e.summary.contains("\"nope\""), "{}", e.summary);
}

#[test]
fn visual_only_keeps_the_frame_and_the_transform_and_withholds_the_rest() {
    let dir = tempdir("visual-only");
    let scene = Scene::random(4, 220, 0.75);
    let source = scene.render(384, 288);
    let candidate = scene.render_warped(384, 288, &similarity(384, 288, 1.0, 0.0, 3.0, 2.0));
    let ctx = context(
        &dir,
        &[
            ("candidate", &candidate, Projection::Flat),
            ("source", &source, Projection::Flat),
        ],
    );
    let projected = Projected::new(
        std::sync::Arc::new(builtin::align::Align::new()),
        "visual_only",
    )
    .unwrap();
    let e = projected
        .call(
            &serde_json::json!({ "image": "candidate", "against": "source" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::Ok);
    assert_eq!(e.images.len(), 1);
    assert_eq!(e.transforms.len(), 1);
    assert_eq!(e.transforms[0].params.len(), 9);
    assert!(e.transforms[0].quality.is_null());
    assert!(e.measurements.is_empty() && e.regions.is_empty());
    assert_eq!(
        e.summary,
        squint_core::tool::projection::VISUAL_ONLY_COMPUTED
    );
    // An unrelated pair under the projection: not available, and no region reaches the checklist.
    let other = Scene::random(5, 220, 0.75).render(384, 288);
    let ctx = context(
        &dir,
        &[
            ("candidate", &other, Projection::Flat),
            ("source", &source, Projection::Flat),
        ],
    );
    let e = projected
        .call(
            &serde_json::json!({ "image": "candidate", "against": "source" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::NotApplicable);
    assert!(e.regions.is_empty() && e.measurements.is_empty());
    assert_eq!(
        e.summary,
        squint_core::tool::projection::VISUAL_ONLY_UNAVAILABLE
    );
}

#[test]
fn a_panorama_pair_registers_by_rotation() {
    let dir = tempdir("sphere");
    let scene = SphereScene::random(30, 400);
    let source = scene.render(512, 256, &IDENTITY);
    let truth = images::rotation(23.0, 4.0, -2.5);
    let candidate = scene.render(512, 256, &truth);
    let ctx = context(
        &dir,
        &[
            ("candidate", &candidate, Projection::Equirectangular),
            ("source", &source, Projection::Equirectangular),
        ],
    );
    let tool = builtin::align::Align::new();
    let e = tool
        .call(
            &serde_json::json!({ "image": "candidate", "against": "source" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::Ok, "{}", e.summary);
    let t = &e.transforms[0];
    assert_eq!(t.kind, "rotation");
    let r: [f64; 9] = t.params.clone().try_into().unwrap();
    // The scene appears at `truth · p` in the candidate; registering the candidate onto the
    // source undoes it, so the transform is the inverse of the truth.
    let inv = images::invert(&truth);
    assert!(
        images::rotation_angle_between(&r, &inv) < 0.1,
        "{:?}",
        images::rotation_angle_between(&r, &inv)
    );
    assert!(
        (measurement(&e, "yaw_deg") + 23.0).abs() < 0.5,
        "{}",
        measurement(&e, "yaw_deg")
    );
    assert_eq!(e.images[0].frame, "aligned:candidate:t1");
    assert_eq!(e.images[0].mapping, Mapping::Rotation(t.params.clone()));
    assert_eq!((e.images[0].width, e.images[0].height), (512, 256));
    // The aligned panorama matches the source.
    let warped = decode(e.images[0].base64.as_ref().unwrap());
    let mut diff = 0.0f64;
    for (p, q) in warped.pixels().zip(source.pixels()) {
        diff += (0..3)
            .map(|k| f64::from(p.0[k].abs_diff(q.0[k])))
            .sum::<f64>()
            / 3.0;
    }
    assert!(
        diff / (512.0 * 256.0) < 3.0,
        "mean difference {}",
        diff / (512.0 * 256.0)
    );
}

/// Through the run: the model aligns the candidate onto the source and marks a violation on the
/// aligned frame; the report records the aligned frame with its homography and the item's root
/// location on the candidate, mapped back through the inverse.
#[test]
fn a_finding_on_an_aligned_frame_maps_back_to_the_candidate() {
    let out = tempdir("run");
    let world = tempdir("run-world");
    let scene = Scene::random(6, 220, 0.75);
    let source = scene.render(384, 288);
    // The candidate is the scene shifted right by a tenth of the width.
    let candidate = scene.render_warped(384, 288, &similarity(384, 288, 1.0, 0.0, 38.4, 0.0));
    source.save(world.join("source.png")).unwrap();
    candidate.save(world.join("candidate.png")).unwrap();
    std::fs::write(
        world.join("job.toml"),
        r#"[job]
use_case = "source-fidelity"
brief = "Restyle the scene; keep the structure."

[[job.images]]
id = "candidate"
role = "candidate"
source = { path = "candidate.png" }

[[job.images]]
id = "source"
role = "source"
source = { path = "source.png" }
"#,
    )
    .unwrap();
    let script = serde_json::json!([
        {
            "tool_calls": [{ "id": "c1", "name": "align", "arguments": { "image": "candidate", "against": "source" } }],
            "stop_reason": "tool_use"
        },
        {
            "tool_calls": [{ "id": "c_submit", "name": "submit_verdict", "arguments": {
                "schema_version": 1,
                "items": [
                    { "id": "chk:ob:SF1@candidate", "disposition": "held", "confidence": "confident", "reason": "scripted", "allowed_by": null, "location": null, "evidence": ["call:1"] },
                    { "id": "chk:ob:SF2@candidate", "disposition": "violation", "confidence": "confident", "reason": "scripted: a stretch in the aligned view", "allowed_by": null,
                      "location": { "frame": "aligned:candidate:t1", "kind": "bbox", "bbox": [0.5, 0.5, 0.6, 0.6], "polygon": null, "yaw": null, "pitch": null, "fov": null }, "evidence": ["call:1"] }
                ],
                "findings": [],
                "opinion": { "verdict": "fail", "reason": "r" }
            } }],
            "stop_reason": "tool_use"
        }
    ]);
    let provider = ScriptedProvider::new(serde_json::from_value(script).unwrap());
    let registry = ToolRegistry::new(builtin::all(), &["align".into()], &[]).unwrap();
    let config = Config::parse(
        &std::fs::read_to_string(root().join("fixtures/config/view.toml"))
            .unwrap()
            .replace("enabled = [\"view\", \"resize\"]", "enabled = [\"align\"]"),
        &["budget.spend_cap_usd=100".into()],
        &|_| None,
    )
    .unwrap();
    let job = Job::load(&world.join("job.toml")).unwrap();
    let o = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run::check(
            &config,
            &job,
            &world,
            &run::Services::new(&provider, &registry),
            &out,
            "test",
        ))
        .unwrap();
    assert_eq!(o.exit_code, 1, "{:?}", o.error);
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    let run0 = &r["runs"][0];
    assert_eq!(
        run0["tool_calls"][0]["status"], "ok",
        "{}",
        run0["tool_calls"][0]
    );
    let frame = &run0["frames"]["aligned:candidate:t1"];
    assert_eq!(frame["root"], "candidate", "{frame}");
    assert_eq!(frame["parent"], "raw:source");
    assert!(frame["mapping"]["homography"].is_array(), "{frame}");
    assert!(
        o.dir
            .join("run-1/frames/aligned_candidate_t1.png")
            .is_file()
    );
    let items = run0["items"].as_array().unwrap();
    let sf2 = items
        .iter()
        .find(|i| i["id"] == "chk:ob:SF2@candidate")
        .unwrap();
    assert_eq!(sf2["root"]["image"], "candidate");
    assert_eq!(sf2["root"]["kind"], "polygon");
    // The box at x 0.5 in the source's grid is at x 0.6 in the candidate, which sits a tenth
    // further right.
    let poly = sf2["root"]["polygon"].as_array().unwrap();
    let x0 = poly[0][0].as_f64().unwrap();
    assert!((x0 - 0.6).abs() < 0.01, "{poly:?}");
}

/// Through the run with a panorama pair: the aligned frame carries a rotation, and a location on
/// it maps back to the candidate through the inverse rotation, shifted by the yaw the candidate
/// was turned by.
#[test]
fn a_finding_on_an_aligned_panorama_maps_back_through_the_rotation() {
    let out = tempdir("run-pano");
    let world = tempdir("run-pano-world");
    let scene = SphereScene::random(31, 400);
    let source = scene.render(512, 256, &IDENTITY);
    // The candidate is the scene turned by a tenth of a circle.
    let candidate = scene.render(512, 256, &images::rotation(36.0, 0.0, 0.0));
    source.save(world.join("source.png")).unwrap();
    candidate.save(world.join("candidate.png")).unwrap();
    std::fs::write(
        world.join("job.toml"),
        r#"[job]
use_case = "source-fidelity"
brief = "Restyle the scene; keep the structure."

[[job.images]]
id = "candidate"
role = "candidate"
source = { path = "candidate.png" }
projection = "equirectangular"

[[job.images]]
id = "source"
role = "source"
source = { path = "source.png" }
projection = "equirectangular"
"#,
    )
    .unwrap();
    let script = serde_json::json!([
        {
            "tool_calls": [{ "id": "c1", "name": "align", "arguments": { "image": "candidate", "against": "source" } }],
            "stop_reason": "tool_use"
        },
        {
            "tool_calls": [{ "id": "c_submit", "name": "submit_verdict", "arguments": {
                "schema_version": 1,
                "items": [
                    { "id": "chk:ob:SF1@candidate", "disposition": "held", "confidence": "confident", "reason": "scripted", "allowed_by": null, "location": null, "evidence": ["call:1"] },
                    { "id": "chk:ob:SF2@candidate", "disposition": "violation", "confidence": "confident", "reason": "scripted: a stretch in the aligned view", "allowed_by": null,
                      "location": { "frame": "aligned:candidate:t1", "kind": "bbox", "bbox": [0.45, 0.45, 0.55, 0.55], "polygon": null, "yaw": null, "pitch": null, "fov": null }, "evidence": ["call:1"] }
                ],
                "findings": [],
                "opinion": { "verdict": "fail", "reason": "r" }
            } }],
            "stop_reason": "tool_use"
        }
    ]);
    let provider = ScriptedProvider::new(serde_json::from_value(script).unwrap());
    let registry = ToolRegistry::new(builtin::all(), &["align".into()], &[]).unwrap();
    let config = Config::parse(
        &std::fs::read_to_string(root().join("fixtures/config/view.toml"))
            .unwrap()
            .replace("enabled = [\"view\", \"resize\"]", "enabled = [\"align\"]"),
        &["budget.spend_cap_usd=100".into()],
        &|_| None,
    )
    .unwrap();
    let job = Job::load(&world.join("job.toml")).unwrap();
    let o = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run::check(
            &config,
            &job,
            &world,
            &run::Services::new(&provider, &registry),
            &out,
            "test",
        ))
        .unwrap();
    assert_eq!(o.exit_code, 1, "{:?}", o.error);
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    let run0 = &r["runs"][0];
    assert_eq!(
        run0["tool_calls"][0]["status"], "ok",
        "{}",
        run0["tool_calls"][0]
    );
    let frame = &run0["frames"]["aligned:candidate:t1"];
    assert_eq!(frame["root"], "candidate", "{frame}");
    assert_eq!(frame["projection"], "equirectangular", "{frame}");
    assert!(frame["mapping"]["rotation"].is_array(), "{frame}");
    let sf2 = run0["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "chk:ob:SF2@candidate")
        .unwrap()
        .clone();
    assert_eq!(sf2["root"]["image"], "candidate");
    assert_eq!(sf2["root"]["kind"], "polygon");
    // A box centred on the source's grid lands a tenth of the width away on the candidate, at the
    // same height: a pure yaw moves longitude only.
    let poly = sf2["root"]["polygon"].as_array().unwrap();
    let n = poly.len() as f64;
    let cx = poly.iter().map(|p| p[0].as_f64().unwrap()).sum::<f64>() / n;
    let cy = poly.iter().map(|p| p[1].as_f64().unwrap()).sum::<f64>() / n;
    assert!(
        ((cx - 0.5).abs() - 0.1).abs() < 0.01 && (cy - 0.5).abs() < 0.01,
        "centre ({cx}, {cy}) of {poly:?}"
    );
}

#[test]
fn a_frame_argument_given_as_an_image_id_resolves_to_its_raw_frame() {
    // The mirror of the image-argument fix: a model that sends `candidate` where a tool wants a
    // frame gets the image's raw frame, and the journal keeps what it sent.
    let out = tempdir("frame-arg");
    let world = tempdir("frame-arg-world");
    let scene = Scene::random(6, 220, 0.75);
    scene
        .render(256, 192)
        .save(world.join("candidate.png"))
        .unwrap();
    scene
        .render(256, 192)
        .save(world.join("source.png"))
        .unwrap();
    std::fs::write(
        world.join("job.toml"),
        r#"[job]
use_case = "source-fidelity"

[[job.images]]
id = "candidate"
role = "candidate"
source = { path = "candidate.png" }

[[job.images]]
id = "source"
role = "source"
source = { path = "source.png" }
"#,
    )
    .unwrap();
    let script = serde_json::json!([
        {
            "tool_calls": [{ "id": "c1", "name": "view", "arguments": { "frame": "candidate", "bbox": [0.1, 0.1, 0.6, 0.6] } }],
            "stop_reason": "tool_use"
        },
        {
            "tool_calls": [{ "id": "c_submit", "name": "submit_verdict", "arguments": {
                "schema_version": 1,
                "items": [
                    { "id": "chk:ob:SF1@candidate", "disposition": "held", "confidence": "confident", "reason": "scripted", "allowed_by": null, "location": null, "evidence": [] },
                    { "id": "chk:ob:SF2@candidate", "disposition": "held", "confidence": "confident", "reason": "scripted", "allowed_by": null, "location": null, "evidence": [] }
                ],
                "findings": [],
                "opinion": { "verdict": "pass", "reason": "r" }
            } }],
            "stop_reason": "tool_use"
        }
    ]);
    let provider = ScriptedProvider::new(serde_json::from_value(script).unwrap());
    let registry = ToolRegistry::new(builtin::all(), &["view".into()], &[]).unwrap();
    let config = Config::parse(
        &std::fs::read_to_string(root().join("fixtures/config/view.toml"))
            .unwrap()
            .replace("enabled = [\"view\", \"resize\"]", "enabled = [\"view\"]"),
        &["budget.spend_cap_usd=100".into()],
        &|_| None,
    )
    .unwrap();
    let job = Job::load(&world.join("job.toml")).unwrap();
    let o = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run::check(
            &config,
            &job,
            &world,
            &run::Services::new(&provider, &registry),
            &out,
            "test",
        ))
        .unwrap();
    assert_eq!(o.exit_code, 0, "{:?}", o.error);
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    let call = &r["runs"][0]["tool_calls"][0];
    assert_eq!(call["status"], "ok", "{call}");
    assert!(call["invalid_args"].is_null(), "{call}");
    assert_eq!(call["arguments"]["frame"], "candidate");
    assert_eq!(
        r["runs"][0]["frames"]["crop:call:1:0"]["parent"],
        "raw:candidate"
    );
}
