//! Task 4.2a: the `view` and `resize` built-ins. A crop equals the region it cuts; a perspective
//! view matches the stored view and the analytic rendering of the same scene; bad arguments are
//! `invalid_args`; a crop frame made in a run maps findings back to the root image.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use image::RgbImage;
use squint_core::config::Config;
use squint_core::frames::{Mapping, sphere};
use squint_core::job::{Job, Projection};
use squint_core::run;
use squint_core::tool::{FrameHandle, ImageHandle, SquintContext, Status, Tool, ToolRegistry};
use squint_testkit::ScriptedProvider;
use squint_testkit::images::{IDENTITY, Scene, SphereScene};
use squint_tools::builtin;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-builtin-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A context holding frames by id, each saved as a PNG in `dir` the way the harness does.
fn context(dir: &Path, frames: &[(&str, &RgbImage, Projection)]) -> SquintContext {
    let mut ctx = SquintContext {
        run_id: "run".into(),
        call_id: "call:1".into(),
        ..Default::default()
    };
    for (id, img, projection) in frames {
        let path = dir.join(format!("{}.png", id.replace(':', "_")));
        img.save(&path).unwrap();
        ctx.frames.insert(
            (*id).to_string(),
            FrameHandle {
                root: id.trim_start_matches("raw:").to_string(),
                projection: *projection,
                parent: None,
                transform: None,
                chain: Vec::new(),
                pixels: ImageHandle {
                    media_type: "image/png".into(),
                    width: img.width(),
                    height: img.height(),
                    projection: *projection,
                    path: Some(path.display().to_string()),
                    base64: None,
                },
            },
        );
    }
    ctx
}

fn decode(b64: &str) -> RgbImage {
    let bytes = squint_core::image::base64_decode(b64).unwrap();
    image::load_from_memory(&bytes).unwrap().to_rgb8()
}

/// The largest per-channel difference between two images of the same size.
fn max_diff(a: &RgbImage, b: &RgbImage) -> u8 {
    assert_eq!(a.dimensions(), b.dimensions());
    a.pixels()
        .zip(b.pixels())
        .flat_map(|(p, q)| p.0.iter().zip(q.0.iter()).map(|(x, y)| x.abs_diff(*y)))
        .max()
        .unwrap_or(0)
}

#[test]
fn view_crop_equals_the_region_it_cuts() {
    let dir = tempdir("crop");
    let img = Scene::random(1, 120, 0.75).render(512, 384);
    let ctx = context(&dir, &[("raw:candidate", &img, Projection::Flat)]);
    let tool = builtin::view::View::new();
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:candidate", "bbox": [0.25, 0.25, 0.75, 0.75] }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::Ok, "{}", e.summary);
    assert_eq!(e.images.len(), 1);
    let out = &e.images[0];
    assert_eq!(out.frame, "crop:call:1:0");
    assert_eq!(out.parent, "raw:candidate");
    assert_eq!(out.mapping, Mapping::Bbox([0.25, 0.25, 0.75, 0.75]));
    assert_eq!((out.width, out.height), (256, 192));
    let expected = image::imageops::crop_imm(&img, 128, 96, 256, 192).to_image();
    assert!(max_diff(&decode(out.base64.as_ref().unwrap()), &expected) <= 1);
    e.check_bounds(&tool.manifest().returns).unwrap();
    assert!(
        e.summary
            .starts_with("Crop [0.250, 0.250, 0.750, 0.750] of raw:candidate"),
        "{}",
        e.summary
    );
}

#[test]
fn view_snaps_a_box_outward_to_whole_pixels_and_caps_the_long_edge() {
    let dir = tempdir("snap");
    let img = Scene::random(2, 60, 0.5).render(2048, 1024);
    let ctx = context(&dir, &[("raw:candidate", &img, Projection::Flat)]);
    let e = builtin::view::View::new()
        .call(
            &serde_json::json!({ "frame": "raw:candidate", "bbox": [0.1001, 0.2, 0.9, 0.8] }),
            &ctx,
        )
        .unwrap();
    let out = &e.images[0];
    // 0.1001 × 2048 = 205.0 snaps down to pixel 205 and 0.9 × 2048 = 1843.2 up to 1844: the
    // mapping records the pixels actually cut; the 1639 px cut is scaled to the 1024 cap.
    let Mapping::Bbox(b) = &out.mapping else {
        panic!("{:?}", out.mapping)
    };
    assert!(
        (b[0] - 205.0 / 2048.0).abs() < 1e-12 && (b[2] - 1844.0 / 2048.0).abs() < 1e-12,
        "{b:?}"
    );
    assert_eq!(out.width, 1024);
    assert_eq!(out.height, 385);
}

#[test]
fn view_perspective_matches_the_stored_view_and_the_analytic_rendering() {
    let dir = tempdir("perspective");
    let fixtures = root().join("fixtures/images/view");
    let scene = SphereScene::random(7, 120);
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        scene
            .render(512, 256, &IDENTITY)
            .save(fixtures.join("pano.png"))
            .unwrap();
    }
    let pano = image::open(fixtures.join("pano.png")).unwrap().to_rgb8();
    let ctx = context(
        &dir,
        &[("raw:candidate", &pano, Projection::Equirectangular)],
    );
    let v = sphere::View {
        yaw: 30.0,
        pitch: 10.0,
        fov: 60.0,
    };
    let e = builtin::view::View::new()
        .call(&serde_json::json!({ "frame": "raw:candidate", "yaw": 30.0, "pitch": 10.0, "fov": 60.0 }), &ctx)
        .unwrap();
    assert_eq!(e.status, Status::Ok, "{}", e.summary);
    let out = &e.images[0];
    assert_eq!(
        out.mapping,
        Mapping::View {
            yaw: 30.0,
            pitch: 10.0,
            fov: 60.0
        }
    );
    let side = builtin::view::view_side(512, 60.0);
    assert_eq!((out.width, out.height), (side, side));
    let got = decode(out.base64.as_ref().unwrap());
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        got.save(fixtures.join("view-yaw30-pitch10-fov60.png"))
            .unwrap();
    }
    let stored = image::open(fixtures.join("view-yaw30-pitch10-fov60.png"))
        .unwrap()
        .to_rgb8();
    assert!(
        max_diff(&got, &stored) <= 2,
        "stored view differs by {}",
        max_diff(&got, &stored)
    );
    // The analytic view: the scene evaluated along each pixel's direction. The tool resamples an
    // 8-bit panorama, so the two differ by interpolation and quantisation only.
    let mut analytic = RgbImage::new(side, side);
    for (u, w, p) in analytic.enumerate_pixels_mut() {
        let d = v.direction(
            (f64::from(u) + 0.5) / f64::from(side),
            (f64::from(w) + 0.5) / f64::from(side),
        );
        *p = image::Rgb(
            scene
                .at(d)
                .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
        );
    }
    let diff = max_diff(&got, &analytic);
    let mean: f64 = got
        .pixels()
        .zip(analytic.pixels())
        .flat_map(|(p, q)| {
            p.0.iter()
                .zip(q.0.iter())
                .map(|(x, y)| f64::from(x.abs_diff(*y)))
        })
        .sum::<f64>()
        / f64::from(side * side * 3);
    // Sharp blob edges resampled from 8-bit pixels differ by more at single pixels; the mean
    // says it is the right view.
    assert!(
        diff <= 32 && mean < 0.5,
        "analytic view: max {diff} mean {mean:.3}"
    );
}

#[test]
fn view_shows_the_same_region_in_two_frames_of_one_projection() {
    let dir = tempdir("also");
    let a = Scene::random(3, 120, 0.75).render(256, 192);
    let b = Scene::random(4, 120, 0.75).render(512, 384);
    let pano = SphereScene::random(5, 40).render(128, 64, &IDENTITY);
    let ctx = context(
        &dir,
        &[
            ("raw:candidate", &a, Projection::Flat),
            ("raw:source", &b, Projection::Flat),
            ("raw:pano", &pano, Projection::Equirectangular),
        ],
    );
    let tool = builtin::view::View::new();
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:candidate", "also": "raw:source", "bbox": [0.5, 0.5, 1.0, 1.0] }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::Ok, "{}", e.summary);
    assert_eq!(e.images.len(), 2);
    assert_eq!(
        (e.images[0].parent.as_str(), e.images[1].parent.as_str()),
        ("raw:candidate", "raw:source")
    );
    assert_eq!(
        (e.images[0].frame.as_str(), e.images[1].frame.as_str()),
        ("crop:call:1:0", "crop:call:1:1")
    );
    assert_eq!((e.images[0].width, e.images[1].width), (128, 256));
    assert!(
        e.summary.contains("and the same of raw:source"),
        "{}",
        e.summary
    );
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:candidate", "also": "raw:pano", "bbox": [0.5, 0.5, 1.0, 1.0] }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::InvalidArgs);
    assert!(e.summary.contains("different projection"), "{}", e.summary);
}

#[test]
fn view_refuses_bad_arguments_as_the_models_mistake() {
    let dir = tempdir("invalid");
    let flat = Scene::random(6, 60, 0.75).render(256, 192);
    let pano = SphereScene::random(6, 40).render(256, 128, &IDENTITY);
    let ctx = context(
        &dir,
        &[
            ("raw:candidate", &flat, Projection::Flat),
            ("raw:pano", &pano, Projection::Equirectangular),
        ],
    );
    let tool = builtin::view::View::new();
    let cases = [
        (
            serde_json::json!({ "frame": "raw:pano", "yaw": 0.0, "pitch": 0.0, "fov": 130.0 }),
            "fov 130",
        ),
        // Angles on a flat frame: the refusal says what to send instead, since a model that is
        // told only that the three go together repeats the same call.
        (
            serde_json::json!({ "frame": "raw:candidate", "yaw": 0.0, "pitch": 0.0, "fov": 60.0 }),
            "flat: send bbox",
        ),
        (
            serde_json::json!({ "frame": "raw:candidate", "yaw": 0.0 }),
            "flat: send bbox",
        ),
        (
            serde_json::json!({ "frame": "raw:pano", "yaw": 180.0, "pitch": 0.0, "fov": 60.0 }),
            "yaw 180",
        ),
        (
            serde_json::json!({ "frame": "raw:pano", "yaw": 0.0, "pitch": 95.0, "fov": 60.0 }),
            "pitch 95",
        ),
        (
            serde_json::json!({ "frame": "raw:pano", "yaw": 0.0, "pitch": 0.0 }),
            "go together",
        ),
        (
            serde_json::json!({ "frame": "raw:pano", "bbox": [0.1, 0.1, 0.5, 0.5], "yaw": 0.0, "pitch": 0.0, "fov": 60.0 }),
            "not both",
        ),
        (serde_json::json!({ "frame": "raw:candidate" }), "give bbox"),
        (
            serde_json::json!({ "frame": "raw:candidate", "bbox": [0.5, 0.5, 0.4, 0.9] }),
            "left < right",
        ),
        // A box past the right edge of a panorama: the seam is crossed by two boxes, never one.
        (
            serde_json::json!({ "frame": "raw:pano", "bbox": [0.9, 0.25, 1.1, 0.75] }),
            "fractions",
        ),
        (
            serde_json::json!({ "frame": "raw:candidate", "bbox": [0.5, 0.5, 0.51, 0.51] }),
            "at least 8×8",
        ),
        (
            serde_json::json!({ "frame": "raw:nope", "bbox": [0.1, 0.1, 0.5, 0.5] }),
            "unknown frame",
        ),
    ];
    for (args, needle) in cases {
        let e = tool.call(&args, &ctx).unwrap();
        assert_eq!(e.status, Status::InvalidArgs, "{args}: {}", e.summary);
        assert!(
            e.summary.contains(needle),
            "{args}: {} should mention {needle:?}",
            e.summary
        );
        assert!(e.images.is_empty());
    }
    // A bbox on a panorama is a plain crop, allowed anywhere it does not wrap (a normalised box
    // cannot cross the seam at x = 0 or 1).
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:pano", "bbox": [0.0, 0.25, 0.25, 0.75] }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::Ok, "{}", e.summary);
}

#[test]
fn resize_scales_down_and_never_up() {
    let dir = tempdir("resize");
    let img = Scene::random(8, 120, 0.75).render(512, 384);
    let ctx = context(&dir, &[("raw:candidate", &img, Projection::Flat)]);
    let tool = builtin::resize::Resize::new();
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:candidate", "long_edge": 256 }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::Ok, "{}", e.summary);
    let out = &e.images[0];
    assert_eq!((out.width, out.height), (256, 192));
    assert_eq!(out.mapping, Mapping::Bbox([0.0, 0.0, 1.0, 1.0]));
    let expected = image::imageops::resize(&img, 256, 192, image::imageops::FilterType::Triangle);
    assert_eq!(
        max_diff(&decode(out.base64.as_ref().unwrap()), &expected),
        0
    );
    e.check_bounds(&tool.manifest().returns).unwrap();
    for (long_edge, needle) in [
        (512u64, "never enlarges"),
        (1024, "never enlarges"),
        (32, "outside [64, 1024]"),
    ] {
        let e = tool
            .call(
                &serde_json::json!({ "frame": "raw:candidate", "long_edge": long_edge }),
                &ctx,
            )
            .unwrap();
        assert_eq!(e.status, Status::InvalidArgs);
        assert!(e.summary.contains(needle), "{}", e.summary);
    }
}

#[test]
fn builtins_register_in_cheap_first_order_and_declare_no_projection() {
    let registry =
        ToolRegistry::new(builtin::all(), &["resize".into(), "view".into()], &[]).unwrap();
    let names: Vec<&str> = registry.manifests().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["resize", "view"], "both cheap; alphabetical");
    for m in registry.manifests() {
        assert!(m.projections.is_empty());
        assert_eq!(m.frames_out, ["crop"]);
    }
}

/// Every built-in's image and frame arguments say they take an image id or a raw frame id
/// (F2): a model that knew only one form sent the other and lost a turn to `invalid_args`.
#[test]
fn every_builtin_image_argument_names_the_image_id_and_the_raw_frame() {
    let tools = builtin::all();
    assert!(!tools.is_empty());
    let gaps: Vec<String> = tools
        .iter()
        .flat_map(|t| squint_testkit::tool::image_argument_gaps(t.manifest()))
        .collect();
    assert!(gaps.is_empty(), "{gaps:#?}");
}

/// Through the run: the model crops the candidate, then marks a violation on the crop; the
/// report records the crop frame with its mapping and the item's root location on the candidate.
#[test]
fn a_crop_made_in_a_run_maps_a_finding_back_to_the_candidate() {
    let out = tempdir("run");
    let script = serde_json::json!([
        {
            "tool_calls": [{ "id": "c1", "name": "view", "arguments": { "frame": "raw:candidate", "bbox": [0.5, 0.5, 1.0, 1.0] } }],
            "stop_reason": "tool_use"
        },
        {
            "tool_calls": [{ "id": "c_submit", "name": "submit_verdict", "arguments": {
                "schema_version": 1,
                "items": [
                    { "id": "chk:ob:SF1@candidate", "disposition": "held", "confidence": "confident", "reason": "scripted", "allowed_by": null, "location": null, "evidence": ["call:1"] },
                    { "id": "chk:ob:SF2@candidate", "disposition": "violation", "confidence": "confident", "reason": "scripted: the corner is stretched", "allowed_by": null,
                      "location": { "frame": "crop:call:1:0", "kind": "bbox", "bbox": [0.0, 0.0, 0.5, 0.5], "polygon": null, "yaw": null, "pitch": null, "fov": null }, "evidence": ["call:1"] }
                ],
                "findings": [],
                "opinion": { "verdict": "fail", "reason": "r" }
            } }],
            "stop_reason": "tool_use"
        }
    ]);
    let provider = ScriptedProvider::new(serde_json::from_value(script).unwrap());
    let registry = ToolRegistry::new(builtin::all(), &["view".into()], &[]).unwrap();
    // The envelope plans for two full-size images coming back from `view`, which the fake prices
    // high, so the cap is raised for the test.
    let config = Config::parse(
        &std::fs::read_to_string(root().join("fixtures/config/view.toml")).unwrap(),
        &["budget.spend_cap_usd=100".into()],
        &|_| None,
    )
    .unwrap();
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let o = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run::check(
            &config,
            &job,
            &root(),
            &run::Services::new(&provider, &registry),
            &out,
            "test",
        ))
        .unwrap();
    assert_eq!(o.exit_code, 1, "{:?}", o.error);
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(o.dir.join("report.json")).unwrap()).unwrap();
    let run0 = &r["runs"][0];
    assert_eq!(run0["tool_calls"][0]["tool"], "view");
    assert_eq!(run0["tool_calls"][0]["status"], "ok");
    let frame = &run0["frames"]["crop:call:1:0"];
    assert_eq!(frame["root"], "candidate");
    assert_eq!(frame["parent"], "raw:candidate");
    assert_eq!(
        frame["mapping"]["bbox"],
        serde_json::json!([0.5, 0.5, 1.0, 1.0])
    );
    assert!(o.dir.join("run-1/frames/crop_call_1_0.png").is_file());
    let items = run0["items"].as_array().unwrap();
    let sf2 = items
        .iter()
        .find(|i| i["id"] == "chk:ob:SF2@candidate")
        .unwrap();
    assert_eq!(sf2["root"]["image"], "candidate");
    assert_eq!(
        sf2["root"]["bbox"],
        serde_json::json!([0.5, 0.5, 0.75, 0.75])
    );
    let _: BTreeMap<String, serde_json::Value> =
        serde_json::from_value(run0["frames"].clone()).unwrap();
}
