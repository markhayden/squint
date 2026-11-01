//! Task 4.3c: `edge_similarity` gives 1 for identical frames and near 0 for a blank against a
//! textured one; `change_map` over a fixture pair equals the stored golden map within one grey
//! level and lights up where the scene changed.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use image::RgbImage;
use squint_core::frames::Mapping;
use squint_core::job::Projection;
use squint_core::tool::{FrameHandle, ImageHandle, SquintContext, Status, Tool};
use squint_testkit::images::{IDENTITY, Scene, SphereScene, low_texture};
use squint_tools::builtin;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-compare-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn context(dir: &Path, frames: &[(&str, &RgbImage)]) -> SquintContext {
    let mut ctx = SquintContext {
        run_id: "run".into(),
        call_id: "call:1".into(),
        ..Default::default()
    };
    for (id, img) in frames {
        let path = dir.join(format!("{}.png", id.replace(':', "_")));
        img.save(&path).unwrap();
        ctx.frames.insert(
            (*id).to_string(),
            FrameHandle {
                root: id.trim_start_matches("raw:").to_string(),
                projection: Projection::Flat,
                parent: None,
                transform: None,
                chain: Vec::new(),
                pixels: ImageHandle {
                    media_type: "image/png".into(),
                    width: img.width(),
                    height: img.height(),
                    projection: Projection::Flat,
                    path: Some(path.display().to_string()),
                    base64: None,
                },
            },
        );
    }
    ctx
}

fn value(e: &squint_core::tool::Envelope, name: &str) -> f64 {
    e.measurements
        .iter()
        .find(|m| m.name == name)
        .unwrap()
        .value
}

#[test]
fn edge_similarity_is_one_for_the_same_frame_and_near_zero_for_blank_against_textured() {
    let dir = tempdir("edge");
    let scene = Scene::random(1, 220, 0.75).render(384, 288);
    let blank = low_texture(384, 288, 1, 0.0);
    let ctx = context(
        &dir,
        &[
            ("raw:source", &scene),
            ("raw:copy", &scene),
            ("raw:blank", &blank),
        ],
    );
    let tool = builtin::compare::EdgeSimilarity::new();
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:source", "against": "raw:copy" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::Ok, "{}", e.summary);
    assert!(
        (value(&e, "edge_similarity") - 1.0).abs() < 1e-6,
        "{}",
        value(&e, "edge_similarity")
    );
    e.check_bounds(&tool.manifest().returns).unwrap();
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:source", "against": "raw:blank" }),
            &ctx,
        )
        .unwrap();
    assert!(
        value(&e, "edge_similarity") < 0.05,
        "{}",
        value(&e, "edge_similarity")
    );
    assert!(e.images.is_empty() && e.regions.is_empty());
    // Mistakes are the model's: the same frame twice, an unknown frame, frames of another shape
    // or projection.
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:source", "against": "raw:source" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::InvalidArgs);
    let wide = Scene::random(9, 60, 0.5).render(384, 192);
    let mut other = context(&dir, &[("raw:wide", &wide)]);
    other.frames.extend(ctx.frames.clone());
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:source", "against": "raw:wide" }),
            &other,
        )
        .unwrap();
    assert_eq!(e.status, Status::InvalidArgs);
    assert!(e.summary.contains("share one grid"), "{}", e.summary);
    let pano_image = SphereScene::random(5, 40).render(384, 192, &IDENTITY);
    let mut pano = context(&dir, &[("raw:pano", &pano_image)]);
    pano.frames.get_mut("raw:pano").unwrap().projection = Projection::Equirectangular;
    pano.frames.extend(ctx.frames.clone());
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:source", "against": "raw:pano" }),
            &pano,
        )
        .unwrap();
    assert_eq!(e.status, Status::InvalidArgs);
    assert!(e.summary.contains("Equirectangular"), "{}", e.summary);
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:source", "against": "raw:nope" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::InvalidArgs);
}

#[test]
fn edge_similarity_survives_a_tone_change_and_drops_for_a_different_scene() {
    let dir = tempdir("edge-tone");
    let scene = Scene::random(2, 220, 0.75);
    let a = scene.render(384, 288);
    let mut toned = a.clone();
    for p in toned.pixels_mut() {
        *p = image::Rgb(p.0.map(|v| ((f64::from(v) / 255.0).powf(1.6) * 220.0) as u8));
    }
    let other = Scene::random(3, 220, 0.75).render(384, 288);
    let ctx = context(
        &dir,
        &[("raw:a", &a), ("raw:toned", &toned), ("raw:other", &other)],
    );
    let tool = builtin::compare::EdgeSimilarity::new();
    let same = value(
        &tool
            .call(
                &serde_json::json!({ "frame": "raw:a", "against": "raw:toned" }),
                &ctx,
            )
            .unwrap(),
        "edge_similarity",
    );
    let different = value(
        &tool
            .call(
                &serde_json::json!({ "frame": "raw:a", "against": "raw:other" }),
                &ctx,
            )
            .unwrap(),
        "edge_similarity",
    );
    assert!(same > 0.9, "toned {same}");
    assert!(different < 0.3, "other {different}");
}

#[test]
fn change_map_equals_the_stored_golden_map_and_lights_up_where_the_scene_changed() {
    let dir = tempdir("change-map");
    let fixtures = root().join("fixtures/images/change-map");
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        let scene = Scene::random(7, 160, 0.75);
        let a = scene.render(384, 288);
        // The same scene with one blob moved and one added: a local change under a global tone
        // shift, which the map must ignore.
        let mut changed = scene.clone();
        changed.blobs[3].x += 0.12;
        // A bright blob (a real luminance change, not a colour swap the map should ignore).
        changed.blobs.push(squint_testkit::images::Blob {
            x: 0.8,
            y: 0.2,
            sigma: 0.015,
            rgb: [0.5, 0.45, 0.4],
        });
        changed.base = [
            scene.base[0] + 0.1,
            scene.base[1] - 0.05,
            scene.base[2] + 0.05,
        ];
        let b = changed.render(384, 288);
        a.save(fixtures.join("a.png")).unwrap();
        b.save(fixtures.join("b.png")).unwrap();
    }
    let a = image::open(fixtures.join("a.png")).unwrap().to_rgb8();
    let b = image::open(fixtures.join("b.png")).unwrap().to_rgb8();
    let ctx = context(&dir, &[("raw:source", &a), ("aligned:candidate:t1", &b)]);
    let tool = builtin::compare::ChangeMap::new();
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:source", "against": "aligned:candidate:t1" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(e.status, Status::Ok, "{}", e.summary);
    e.check_bounds(&tool.manifest().returns).unwrap();
    assert_eq!(e.images.len(), 1);
    assert!(e.regions.is_empty());
    let img = &e.images[0];
    assert_eq!(img.frame, "crop:call:1:0");
    assert_eq!(img.parent, "raw:source");
    assert_eq!(img.mapping, Mapping::Bbox([0.0, 0.0, 1.0, 1.0]));
    assert_eq!((img.width, img.height), (384, 288));
    let map = image::load_from_memory(
        &squint_core::image::base64_decode(img.base64.as_ref().unwrap()).unwrap(),
    )
    .unwrap()
    .to_rgb8();
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        map.save(fixtures.join("golden.png")).unwrap();
    }
    let golden = image::open(fixtures.join("golden.png")).unwrap().to_rgb8();
    let worst = map
        .pixels()
        .zip(golden.pixels())
        .map(|(p, q)| p.0[0].abs_diff(q.0[0]))
        .max()
        .unwrap();
    assert!(worst <= 1, "golden differs by {worst}");
    // Bright where the blob was added (around 0.8, 0.2), dark in an untouched corner.
    let mean = |x0: u32, y0: u32, x1: u32, y1: u32| {
        let mut s = 0.0;
        for y in y0..y1 {
            for x in x0..x1 {
                s += f64::from(map.get_pixel(x, y).0[0]);
            }
        }
        s / f64::from((x1 - x0) * (y1 - y0))
    };
    // Scene (0.8, 0.2) is pixel (307, 77) at 384 wide.
    // The map shows changed structure: the new blob's edges light up while its smooth interior
    // and the untouched corner stay dark.
    let added = mean(295, 65, 320, 90);
    let peak = (295..320)
        .flat_map(|x| (65..90).map(move |y| (x, y)))
        .map(|(x, y)| map.get_pixel(x, y).0[0])
        .max()
        .unwrap();
    let quiet = mean(8, 230, 48, 270);
    assert!(
        added > 12.0 && peak > 90 && quiet < 12.0,
        "added {added} peak {peak} quiet {quiet}"
    );
    // Only the new blob's edges cross one unit: a small share of the frame, and not none.
    let fraction = value(&e, "changed_fraction");
    assert!(fraction > 0.0 && fraction < 0.05, "{fraction}");
    // The same frame against itself is all dark.
    let ctx = context(&dir, &[("raw:source", &a), ("raw:copy", &a)]);
    let e = tool
        .call(
            &serde_json::json!({ "frame": "raw:source", "against": "raw:copy" }),
            &ctx,
        )
        .unwrap();
    assert_eq!(value(&e, "changed_fraction"), 0.0);
}
