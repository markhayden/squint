//! Task 4.0: the alignment estimators recover known transforms on synthetic scenes rendered
//! analytically (no resampling error in the ground truth), refuse unrelated, low-texture and
//! degenerate pairs, and reproduce the committed failure fixtures' outcomes.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use image::RgbImage;
use squint_testkit::images::{
    self, IDENTITY, Scene, SphereScene, low_texture, mirrored, perspective, rotation,
    rotation_angle_between, similarity,
};
use squint_tools::align::{self, Options, Status, align_flat, align_sphere};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Pixel errors of an estimate against the truth: at the centre, the worst corner, and the
/// rotation angle in degrees from the linear part.
fn errors(est: &[f64; 9], truth: &[f64; 9], w: u32, h: u32) -> (f64, f64, f64) {
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    let dist = |x: f64, y: f64| {
        let (ax, ay) = align::apply(est, x, y);
        let (bx, by) = align::apply(truth, x, y);
        ((ax - bx).powi(2) + (ay - by).powi(2)).sqrt()
    };
    let centre = dist(cx, cy);
    let corners = [
        (0.0, 0.0),
        (f64::from(w), 0.0),
        (0.0, f64::from(h)),
        (f64::from(w), f64::from(h)),
    ]
    .iter()
    .map(|&(x, y)| dist(x, y))
    .fold(0.0, f64::max);
    let angle = |m: &[f64; 9]| m[3].atan2(m[0]).to_degrees();
    (centre, corners, (angle(est) - angle(truth)).abs())
}

/// A tone change a restyle might apply: per-channel gamma and offset.
fn restyled(img: &RgbImage) -> RgbImage {
    let mut out = img.clone();
    for p in out.pixels_mut() {
        let f = |v: u8, gamma: f64, offset: f64| {
            (((f64::from(v) / 255.0).powf(gamma) + offset).clamp(0.0, 1.0) * 255.0).round() as u8
        };
        *p = image::Rgb([
            f(p.0[0], 1.4, 0.05),
            f(p.0[1], 0.8, -0.03),
            f(p.0[2], 1.1, 0.1),
        ]);
    }
    out
}

fn flat_case(
    seed: u64,
    w: u32,
    h: u32,
    truth: &[f64; 9],
    restyle: bool,
) -> (align::FlatAlignment, (f64, f64, f64)) {
    let scene = Scene::random(seed, 220, f64::from(h) / f64::from(w));
    let source = scene.render(w, h);
    let mut candidate = scene.render_warped(w, h, truth);
    if restyle {
        candidate = restyled(&candidate);
    }
    let a = align_flat(&source, &candidate, &Options::default());
    let e = errors(&a.homography, truth, w, h);
    (a, e)
}

#[test]
fn a_translation_is_recovered_to_a_tenth_of_a_pixel() {
    let truth = similarity(512, 384, 1.0, 0.0, 7.3, -4.6);
    let (a, (centre, corner, angle)) = flat_case(1, 512, 384, &truth, false);
    assert_eq!(a.status, Status::Registered, "{a:?}");
    assert!(
        centre < 0.1 && corner < 0.15,
        "centre {centre} corner {corner} {a:?}"
    );
    assert!(angle < 0.02, "angle {angle}");
    assert!(
        a.quality.ncc > 0.95 && a.quality.inlier_fraction > 0.95,
        "{:?}",
        a.quality
    );
}

/// The ADR 0014 tolerance at the width its table quotes, in release builds only: a 2048 px
/// pyramid takes minutes under debug assertions.
#[cfg(not(debug_assertions))]
#[test]
fn at_2048_a_rotation_with_scale_is_recovered_within_the_recorded_tolerance() {
    let (w, h) = (2048, 1536);
    let truth = similarity(w, h, 1.06, 6.5, -3.0, 2.5);
    let (a, (centre, corner, angle)) = flat_case(2148, w, h, &truth, false);
    assert_eq!(a.status, Status::Registered, "{a:?}");
    assert!(
        centre < 0.05 && corner < 0.1,
        "centre {centre} corner {corner} {a:?}"
    );
    assert!(angle < 0.01, "angle {angle}");
}

#[test]
fn rotation_scale_and_translation_are_recovered_together() {
    let truth = similarity(512, 384, 1.06, 6.5, -3.0, 2.5);
    let (a, (centre, corner, angle)) = flat_case(2, 512, 384, &truth, false);
    assert_eq!(a.status, Status::Registered, "{a:?}");
    assert!(
        centre < 0.1 && corner < 0.2,
        "centre {centre} corner {corner}"
    );
    assert!(angle < 0.05, "angle {angle}");
}

#[test]
fn a_perspective_term_is_recovered() {
    let base = similarity(512, 512, 1.0, -4.0, 5.0, 3.0);
    let truth = perspective(512, 512, &base, 0.00015, -0.0001);
    let (a, (centre, corner, _)) = flat_case(3, 512, 512, &truth, false);
    assert_eq!(a.status, Status::Registered, "{a:?}");
    assert!(
        centre < 0.1 && corner < 0.3,
        "centre {centre} corner {corner}"
    );
}

#[test]
fn a_restyled_candidate_registers_as_well_as_the_original() {
    let truth = similarity(512, 384, 1.0, 3.0, 4.0, -6.0);
    let (a, (centre, corner, _)) = flat_case(4, 512, 384, &truth, true);
    assert_eq!(a.status, Status::Registered, "{a:?}");
    assert!(
        centre < 0.15 && corner < 0.3,
        "centre {centre} corner {corner}"
    );
}

#[test]
fn a_candidate_at_twice_the_resolution_is_mapped_through_the_scale() {
    let scene = Scene::random(5, 220, 0.75);
    let source = scene.render(256, 192);
    // The same scene rendered at double size, shifted: pixel (x, y) of the source is at
    // (2x + 6, 2y - 4) of the candidate.
    let truth = [2.0, 0.0, 6.0, 0.0, 2.0, -4.0, 0.0, 0.0, 1.0];
    let candidate = scene.render_warped_from(256, 512, 384, &truth);
    let a = align_flat(&source, &candidate, &Options::default());
    assert_eq!(a.status, Status::Registered, "{a:?}");
    let (centre, corner, _) = errors(&a.homography, &truth, 256, 192);
    assert!(
        centre < 0.2 && corner < 0.4,
        "centre {centre} corner {corner}"
    );
}

/// A refinement that cannot settle is a numerical outcome, told apart from a pair that does not
/// register: with no iterations allowed nothing converges, and no transform is claimed.
#[test]
fn a_refinement_that_does_not_converge_is_not_converged_rather_than_not_registered() {
    // An identical pair agrees under the coarse transform, so the only thing missing is a
    // refinement that settled: not converged, no transform claimed.
    let scene = Scene::random(1, 220, 0.75);
    let source = scene.render(256, 192);
    let opts = Options {
        max_iterations: 0,
        ..Options::default()
    };
    let a = align_flat(&source, &source, &opts);
    assert_eq!(a.status, Status::NotConverged, "{a:?}");
    assert_eq!(
        a.homography[2], 0.0,
        "no transform claimed: {:?}",
        a.homography
    );
    let pano = SphereScene::random(30, 400).render(128, 64, &IDENTITY);
    let s = align_sphere(&pano, &pano, &opts);
    assert_eq!(s.status, Status::NotConverged, "{s:?}");
    assert_eq!(s.quality.overlap, 0.0);
}

#[test]
fn unrelated_scenes_are_not_registered() {
    let a = Scene::random(10, 220, 0.75).render(384, 288);
    let b = Scene::random(11, 220, 0.75).render(384, 288);
    let r = align_flat(&a, &b, &Options::default());
    assert_eq!(r.status, Status::NotRegistered, "{r:?}");
    assert!(r.quality.ncc < 0.3, "{:?}", r.quality);
}

#[test]
fn a_graded_image_is_low_texture_and_noise_never_registers() {
    let a = low_texture(256, 192, 1, 0.0);
    let b = low_texture(256, 192, 2, 0.0);
    let r = align_flat(&a, &b, &Options::default());
    assert_eq!(r.status, Status::LowTexture, "{r:?}");
    let a = low_texture(256, 192, 3, 0.02);
    let b = low_texture(256, 192, 4, 0.02);
    let r = align_flat(&a, &b, &Options::default());
    assert_ne!(r.status, Status::Registered, "{r:?}");
}

#[test]
fn degenerate_transforms_are_refused() {
    let scene = Scene::random(20, 220, 1.0);
    let source = scene.render(320, 320);
    // A mirror image: no homography reachable from the identity registers it.
    let r = align_flat(&source, &mirrored(&source), &Options::default());
    assert_eq!(r.status, Status::NotRegistered, "mirror {r:?}");
    // A quarter turn: outside the viewpoint the tool assumes.
    let quarter = scene.render_warped(320, 320, &similarity(320, 320, 1.0, 90.0, 0.0, 0.0));
    let r = align_flat(&source, &quarter, &Options::default());
    assert_eq!(r.status, Status::NotRegistered, "quarter turn {r:?}");
    // Mostly out of frame: too little overlap to vouch for.
    let far = scene.render_warped(320, 320, &similarity(320, 320, 1.0, 0.0, 270.0, 0.0));
    let r = align_flat(&source, &far, &Options::default());
    assert_eq!(r.status, Status::NotRegistered, "far shift {r:?}");
}

#[test]
fn a_sphere_rotation_is_recovered_to_a_tenth_of_a_degree() {
    let scene = SphereScene::random(30, 400);
    let source = scene.render(256, 128, &IDENTITY);
    let truth = rotation(23.0, 4.0, -2.5);
    let candidate = scene.render(256, 128, &truth);
    let a = align_sphere(&source, &candidate, &Options::default());
    assert_eq!(a.status, Status::Registered, "{a:?}");
    let err = rotation_angle_between(&a.rotation, &truth);
    assert!(err < 0.1, "error {err}° {a:?}");
    assert!((a.yaw_deg - 23.0).abs() < 0.2, "yaw {}", a.yaw_deg);
}

#[test]
fn unrelated_spheres_are_not_registered() {
    let a = SphereScene::random(31, 400).render(256, 128, &IDENTITY);
    let b = SphereScene::random(32, 400).render(256, 128, &IDENTITY);
    let r = align_sphere(&a, &b, &Options::default());
    assert_eq!(r.status, Status::NotRegistered, "{r:?}");
}

/// The committed failure fixtures (`fixtures/images/align/`), regenerated with
/// `UPDATE_FIXTURES=1`, each give the outcome `cases.toml` records.
#[test]
fn the_failure_fixtures_give_their_recorded_outcomes() {
    let dir = root().join("fixtures/images/align");
    let cases: toml::Value =
        toml::from_str(&std::fs::read_to_string(dir.join("cases.toml")).unwrap()).unwrap();
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        let scene = Scene::random(40, 220, 0.75);
        let square = Scene::random(41, 220, 1.0);
        let files: Vec<(&str, RgbImage)> = vec![
            ("scene", scene.render(256, 192)),
            ("square", square.render(256, 256)),
            ("unrelated", Scene::random(42, 220, 0.75).render(256, 192)),
            ("gradient-a", low_texture(256, 192, 1, 0.0)),
            ("gradient-b", low_texture(256, 192, 2, 0.0)),
            ("mirror", mirrored(&square.render(256, 256))),
            (
                "quarter-turn",
                square.render_warped(256, 256, &similarity(256, 256, 1.0, 90.0, 0.0, 0.0)),
            ),
            (
                "far-shift",
                scene.render_warped(256, 192, &similarity(256, 192, 1.0, 0.0, 215.0, 0.0)),
            ),
            (
                "registered",
                restyled(&scene.render_warped(
                    256,
                    192,
                    &similarity(256, 192, 1.02, 2.0, 3.0, -2.0),
                )),
            ),
        ];
        for (name, img) in &files {
            img.save(dir.join(format!("{name}.png"))).unwrap();
        }
    }
    let mut seen = 0;
    for case in cases["case"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let expected = case["status"].as_str().unwrap();
        let load = |key: &str| {
            image::open(dir.join(case[key].as_str().unwrap()))
                .unwrap()
                .to_rgb8()
        };
        let r = align_flat(&load("source"), &load("candidate"), &Options::default());
        let got = serde_json::to_value(r.status).unwrap();
        assert_eq!(
            got,
            serde_json::Value::String(expected.into()),
            "{name}: {r:?}"
        );
        seen += 1;
    }
    assert_eq!(seen, 6);
}

/// The measurement behind ADR 0014, run by hand in release mode:
/// `cargo test -p squint-tools --release --test align measure -- --ignored --nocapture`.
#[test]
#[ignore = "prints the tolerance table for ADR 0014; slow in debug"]
fn measure_tolerances() {
    println!("| Case | Width | Centre px | Corner px | Angle ° | NCC | Inliers | Iterations |");
    println!("|---|---|---|---|---|---|---|---|");
    for &w in &[512u32, 1024, 2048] {
        let h = w * 3 / 4;
        let cases: Vec<(&str, [f64; 9], bool)> = vec![
            (
                "translation",
                similarity(w, h, 1.0, 0.0, 0.0143 * f64::from(w), -0.009 * f64::from(w)),
                false,
            ),
            (
                "rotation 6.5°, scale 1.06",
                similarity(w, h, 1.06, 6.5, -3.0, 2.5),
                false,
            ),
            ("rotation 18°", similarity(w, h, 1.0, 18.0, 0.0, 0.0), false),
            (
                "perspective",
                perspective(
                    w,
                    h,
                    &similarity(w, h, 1.0, -4.0, 5.0, 3.0),
                    0.08 / f64::from(w),
                    -0.05 / f64::from(w),
                ),
                false,
            ),
            (
                "restyled, rotation 3°",
                similarity(w, h, 1.0, 3.0, 4.0, -6.0),
                true,
            ),
        ];
        for (name, truth, restyle) in cases {
            let (a, (centre, corner, angle)) = flat_case(100 + u64::from(w), w, h, &truth, restyle);
            println!(
                "| {name} | {w} | {centre:.3} | {corner:.3} | {angle:.4} | {:.3} | {:.3} | {} ({:?}) |",
                a.quality.ncc, a.quality.inlier_fraction, a.iterations, a.status
            );
        }
    }
    println!();
    println!("| Sphere | Width | Error ° | NCC | Inliers | Iterations |");
    println!("|---|---|---|---|---|---|");
    for &w in &[256u32, 512, 1024, 2048] {
        let scene = SphereScene::random(30, 600);
        let source = scene.render(w, w / 2, &IDENTITY);
        for (name, truth) in [
            ("yaw 23°, pitch 4°, roll -2.5°", rotation(23.0, 4.0, -2.5)),
            ("yaw 150°", rotation(150.0, 0.0, 0.0)),
            ("pitch 9°", rotation(0.0, 9.0, 0.0)),
        ] {
            let candidate = scene.render(w, w / 2, &truth);
            let a = align_sphere(&source, &candidate, &Options::default());
            println!(
                "| {name} | {w} | {:.4} | {:.3} | {:.3} | {} ({:?}) |",
                rotation_angle_between(&a.rotation, &truth),
                a.quality.ncc,
                a.quality.inlier_fraction,
                a.iterations,
                a.status
            );
        }
    }
    // Real generated images when the public sets are fetched: resampled, so the truth carries
    // interpolation; errors include that.
    let real = root().join("evalsets/brief-compliance/stills-v1/items/bakery-asked/candidate.png");
    if let Ok(img) = image::open(&real) {
        let src = img.to_rgb8();
        println!();
        println!(
            "| Real image (bakery-asked, resampled) | Width | Centre px | Corner px | Angle ° | NCC | Inliers |"
        );
        println!("|---|---|---|---|---|---|---|");
        for &w in &[1024u32, 2048] {
            let source = image::imageops::resize(&src, w, w, image::imageops::FilterType::Triangle);
            for (name, truth) in [
                ("translation", similarity(w, w, 1.0, 0.0, 9.3, -5.1)),
                (
                    "rotation 5°, scale 1.03",
                    similarity(w, w, 1.03, 5.0, 2.0, 1.0),
                ),
            ] {
                let candidate = resample(&source, &truth);
                let a = align_flat(&source, &candidate, &Options::default());
                let (centre, corner, angle) = errors(&a.homography, &truth, w, w);
                println!(
                    "| {name} | {w} | {centre:.3} | {corner:.3} | {angle:.4} | {:.3} | {:.3} |",
                    a.quality.ncc, a.quality.inlier_fraction
                );
            }
        }
    }
}

/// Bilinear resampling of an image through a homography (output pixel at `h(x)` shows `x`).
fn resample(img: &RgbImage, h: &[f64; 9]) -> RgbImage {
    let inv = images::invert(h);
    let (w, hh) = img.dimensions();
    let mut out = RgbImage::new(w, hh);
    for (u, v, p) in out.enumerate_pixels_mut() {
        let (sx, sy) = images::apply(&inv, f64::from(u) + 0.5, f64::from(v) + 0.5);
        let (fx, fy) = (sx - 0.5, sy - 0.5);
        if fx < 0.0 || fy < 0.0 || fx >= f64::from(w - 1) || fy >= f64::from(hh - 1) {
            *p = image::Rgb([128, 128, 128]);
            continue;
        }
        let (x0, y0) = (fx as u32, fy as u32);
        let (ax, ay) = (fx - f64::from(x0), fy - f64::from(y0));
        let channel = |k: usize| {
            let g = |x: u32, y: u32| f64::from(img.get_pixel(x, y).0[k]);
            let top = g(x0, y0) * (1.0 - ax) + g(x0 + 1, y0) * ax;
            let bottom = g(x0, y0 + 1) * (1.0 - ax) + g(x0 + 1, y0 + 1) * ax;
            (top * (1.0 - ay) + bottom * ay).round() as u8
        };
        *p = image::Rgb([channel(0), channel(1), channel(2)]);
    }
    out
}
