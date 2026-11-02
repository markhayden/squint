//! Task 2.3: preparation is deterministic, fits the native size exactly, keeps PNG with alpha,
//! strips metadata; preflight refuses a conversation at the many-image threshold before sending.
#![allow(clippy::unwrap_used)]

use std::io::Cursor;

use image::{DynamicImage, ImageEncoder, Rgb, Rgba};
use squint_core::image::Prepared;
use squint_core::image::{DecodeBounds, ImagePlan, base64_encode, fit, load};
use squint_core::job::{ImageSource, ImageSpec, Role};
use squint_core::provider::{ContentBlock, Message, MessageRole, TurnRequest};
use squint_providers::catalog::{Catalog, Images};
use squint_providers::prepare::{PREP_VERSION, Preparer, preflight};
use std::collections::BTreeMap;

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn limits(model: &str) -> Images {
    Catalog::shipped()
        .unwrap()
        .model(model)
        .unwrap()
        .images
        .clone()
}

fn spec(id: &str, path: &str) -> ImageSpec {
    ImageSpec {
        id: id.into(),
        role: Role::Candidate,
        label: None,
        projection: None,
        expect: None,
        source: ImageSource::Path {
            path: path.into(),
            sha256: None,
        },
    }
}

fn gradient(w: u32, h: u32) -> DynamicImage {
    DynamicImage::ImageRgb8(image::RgbImage::from_fn(w, h, |x, y| {
        Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
    }))
}

#[test]
fn prepare_is_deterministic_and_fits_the_native_size_exactly() {
    // The real native size (2576) costs seconds of Lanczos in debug builds; a smaller one
    // exercises the same path.
    let mut lim = limits("claude-opus-5-5");
    lim.native_long_edge_px = 320;
    let p = Preparer::new(&lim, None, ImagePlan::Standard);
    let big = gradient(500, 375);
    let a = p.prepare("candidate", &big).unwrap();
    let b = p.prepare("candidate", &big).unwrap();
    assert_eq!(a, b, "identical bytes on repeat");
    assert_eq!(a.bytes, b.bytes);
    assert_eq!(
        (a.width, a.height),
        (320, 240),
        "exact native long edge, aspect kept"
    );
    assert!(a.resized());
    assert_eq!(a.media_type, "image/jpeg");
    assert_eq!(a.prep_version, PREP_VERSION);
    assert_eq!(a.sha256.len(), 64);
    let decoded = image::load_from_memory(&a.bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (320, 240));
    // Never upscaled; the config may only lower the target.
    let small = p.prepare("s", &gradient(64, 48)).unwrap();
    assert!(!small.resized());
    let lower = Preparer::new(&lim, Some(160), ImagePlan::Standard)
        .prepare("c", &big)
        .unwrap();
    assert_eq!((lower.width, lower.height), (160, 120));
    assert_eq!(
        Preparer::new(&lim, Some(99_999), ImagePlan::Standard).long_edge(),
        320
    );
    assert_eq!(fit(3000, 4000, 1536), (1152, 1536));
    assert_eq!(fit(4000, 3000, 2576), (2576, 1932));
}

/// F1: a many-image plan lowers the long edge further and is recorded in `prep_version`; the
/// standard plan is the preparation as it always was.
#[test]
fn a_many_image_plan_fits_the_smaller_edge_and_names_itself() {
    let mut lim = limits("claude-opus-5-5");
    lim.native_long_edge_px = 320;
    let big = gradient(500, 375);
    let usual = Preparer::new(&lim, None, ImagePlan::Standard)
        .prepare("c", &big)
        .unwrap();
    let many = Preparer::new(&lim, None, ImagePlan::Many { long_edge: 200 });
    assert_eq!(many.long_edge(), 200);
    let smaller = many.prepare("c", &big).unwrap();
    assert_eq!((smaller.width, smaller.height), (200, 150));
    assert_eq!(smaller.prep_version, format!("{PREP_VERSION}+many200"));
    assert_ne!(smaller.sha256, usual.sha256);
    // The config's lower edge still wins; the plan is recorded all the same.
    let lower = Preparer::new(&lim, Some(160), ImagePlan::Many { long_edge: 200 })
        .prepare("c", &big)
        .unwrap();
    assert_eq!((lower.width, lower.height), (160, 120));
    assert_eq!(lower.prep_version, format!("{PREP_VERSION}+many200"));
    // An image already within the edge is never upscaled.
    let small = many.prepare("s", &gradient(64, 48)).unwrap();
    assert!(!small.resized());
}

/// The catalog hands resolve each model's per-request count and many-image rule.
#[test]
fn catalog_prices_carry_the_models_request_image_limits() {
    use squint_core::image::{ManyImages, RequestImageLimits};
    use squint_core::ledger::Pricing;
    let catalog = Catalog::shipped().unwrap();
    assert_eq!(
        catalog.model("claude-opus-5-5").unwrap().image_limits(),
        Some(RequestImageLimits {
            max_per_request: 600,
            many: Some(ManyImages {
                threshold: 20,
                max_long_edge: 2000
            }),
        })
    );
    assert_eq!(
        catalog.model("gemini-4-argon").unwrap().image_limits(),
        Some(RequestImageLimits {
            max_per_request: 3600,
            many: None,
        })
    );
}

#[test]
fn prepare_keeps_png_with_alpha_and_drops_metadata() {
    let lim = limits("gemini-4-argon");
    let p = Preparer::new(&lim, None, ImagePlan::Standard);
    // An RGBA image with a transparent corner, loaded through the canonical loader.
    let rgba = image::RgbaImage::from_fn(80, 40, |x, y| {
        Rgba([x as u8, y as u8, 7, if x < 20 && y < 20 { 0 } else { 255 }])
    });
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(Cursor::new(&mut png))
        .write_image(rgba.as_raw(), 80, 40, image::ExtendedColorType::Rgba8)
        .unwrap();
    let canonical = load(
        &ImageSpec {
            id: "alpha".into(),
            role: Role::Candidate,
            label: None,
            projection: None,
            expect: None,
            source: ImageSource::Base64 {
                base64: base64_encode(&png),
                media_type: "image/png".into(),
            },
        },
        &root(),
        &DecodeBounds::default(),
    )
    .unwrap();
    assert!(canonical.has_alpha && canonical.alpha.is_some());
    let prepared = p.prepare_canonical(&canonical).unwrap();
    assert_eq!(prepared.media_type, "image/png");
    let back = image::load_from_memory(&prepared.bytes).unwrap();
    assert!(back.color().has_alpha());
    assert_eq!(
        back.to_rgba8().get_pixel(0, 0).0[3],
        0,
        "transparency survives"
    );
    assert_eq!(back.to_rgba8().get_pixel(79, 39).0[3], 255);
    // An opaque PNG becomes JPEG; the EXIF orientation fixture leaves no EXIF or XMP behind.
    let rotated = load(
        &spec("r", "fixtures/images/exif-rotated-64x48.png"),
        &root(),
        &DecodeBounds::default(),
    )
    .unwrap();
    assert!(!rotated.has_alpha);
    let prepared = p.prepare_canonical(&rotated).unwrap();
    assert_eq!(prepared.media_type, "image/jpeg");
    assert_eq!(
        (prepared.width, prepared.height),
        (48, 64),
        "prepared upright"
    );
    for marker in [
        b"Exif".as_slice(),
        b"eXIf",
        b"<x:xmpmeta",
        b"GPano",
        b"iTXt",
        b"tEXt",
    ] {
        assert!(
            !prepared.bytes.windows(marker.len()).any(|w| w == marker),
            "{:?} left behind",
            String::from_utf8_lossy(marker)
        );
    }
    let pano = load(
        &spec("p", "fixtures/images/pano-gpano-128x64.png"),
        &root(),
        &DecodeBounds::default(),
    )
    .unwrap();
    let prepared = p.prepare_canonical(&pano).unwrap();
    assert!(!prepared.bytes.windows(5).any(|w| w == b"GPano"));
}

fn fake_prepared(id: &str, side: u32, bytes: usize) -> Prepared {
    Prepared {
        id: id.into(),
        media_type: "image/jpeg".into(),
        width: side,
        height: side,
        source_width: side,
        source_height: side,
        sha256: "0".repeat(64),
        prep_version: PREP_VERSION.into(),
        bytes: vec![0; bytes],
    }
}

/// A conversation: the first look with the job images, then tool results carrying crops.
fn conversation(ids: &[String]) -> Vec<Message> {
    let mut history = vec![Message {
        role: MessageRole::User,
        content: vec![
            ContentBlock::Text {
                text: "look".into(),
            },
            ContentBlock::Image {
                image_id: ids[0].clone(),
            },
        ],
    }];
    for (n, id) in ids.iter().enumerate().skip(1) {
        history.push(Message {
            role: MessageRole::Assistant,
            content: vec![ContentBlock::ToolUse {
                id: format!("call:{n}"),
                name: "regions".into(),
                arguments: serde_json::json!({ "image": "candidate" }),
            }],
        });
        history.push(Message {
            role: MessageRole::User,
            content: vec![ContentBlock::ToolResult {
                tool_use_id: format!("call:{n}"),
                content: "one region".into(),
                is_error: false,
                images: vec![id.clone()],
            }],
        });
    }
    history
}

fn request<'a>(history: &'a [Message], images: &'a BTreeMap<String, Prepared>) -> TurnRequest<'a> {
    TurnRequest {
        system: "s",
        history,
        tools: &[],
        max_output_tokens: 1000,
        images,
    }
}

#[test]
fn preflight_counts_tool_images_and_fails_at_the_many_image_threshold_before_sending() {
    let lim = limits("claude-opus-5-5");
    let ids: Vec<String> = (0..21)
        .map(|i| {
            if i == 0 {
                "candidate".to_string()
            } else {
                format!("crop:call:{i}:1")
            }
        })
        .collect();
    let history = conversation(&ids);
    let mut big = BTreeMap::new();
    let mut small = BTreeMap::new();
    for id in &ids {
        big.insert(id.clone(), fake_prepared(id, 2100, 10_000));
        small.insert(id.clone(), fake_prepared(id, 1900, 10_000));
    }
    // 21 images over the threshold of 20: the one over 2000 pixels is refused with the rule.
    let err = preflight(&lim, &request(&history, &big)).unwrap_err();
    assert_eq!(err.class(), "image_limit");
    assert!(
        err.to_string().contains("more than 20 images (21 here)"),
        "{err}"
    );
    // The same count under 2000 pixels passes, and tool-result images were counted.
    let ok = preflight(&lim, &request(&history, &small)).unwrap();
    assert_eq!(ok.images, 21);
    assert_eq!(ok.image_bytes, 210_000);
    // Twenty images at 2100 pixels pass: the stricter rule applies only above the threshold.
    let twenty = conversation(&ids[..20]);
    assert_eq!(preflight(&lim, &request(&twenty, &big)).unwrap().images, 20);
}

/// One image over the model's per-request count is refused before sending, whatever its size.
#[test]
fn preflight_refuses_one_image_over_the_per_request_count() {
    let lim = limits("claude-haiku-5-5");
    let count = lim.max_per_request + 1;
    let ids: Vec<String> = (0..count).map(|i| format!("img{i}")).collect();
    let history = conversation(&ids);
    let mut set = BTreeMap::new();
    for id in &ids {
        set.insert(id.clone(), fake_prepared(id, 64, 1_000));
    }
    let err = preflight(&lim, &request(&history, &set)).unwrap_err();
    assert!(
        err.to_string().contains(&format!(
            "{count} images in the request exceed the model's limit of {} per request",
            lim.max_per_request
        )),
        "{err}"
    );
}

#[test]
fn preflight_enforces_count_bytes_dimensions_and_formats() {
    let lim = limits("gemini-4-argon");
    let ids: Vec<String> = (0..3).map(|i| format!("img{i}")).collect();
    let history = conversation(&ids);
    let mut set = BTreeMap::new();
    for id in &ids {
        set.insert(id.clone(), fake_prepared(id, 1536, 8_000_000));
    }
    let err = preflight(&lim, &request(&history, &set)).unwrap_err();
    assert!(
        err.to_string()
            .contains("over the model's limit of 20000000 bytes"),
        "{err}"
    );
    let mut set = BTreeMap::new();
    for id in &ids {
        set.insert(id.clone(), fake_prepared(id, 1536, 1_000));
    }
    let mut bmp = fake_prepared("img1", 1536, 1_000);
    bmp.media_type = "image/bmp".into();
    set.insert("img1".into(), bmp);
    let err = preflight(&lim, &request(&history, &set)).unwrap_err();
    assert!(err.to_string().contains("image/bmp is not among"), "{err}");
    // An image the request names but nobody prepared is an internal error, not a limit.
    let missing = BTreeMap::new();
    assert_eq!(
        preflight(&lim, &request(&history, &missing))
            .unwrap_err()
            .class(),
        "resolve"
    );
    // Per-side limit (OpenAI) and per-image bytes (Anthropic).
    let openai = limits("gpt-6.1-sol");
    let mut set = BTreeMap::new();
    for id in &ids {
        set.insert(id.clone(), fake_prepared(id, 70_000, 1_000));
    }
    assert!(
        preflight(&openai, &request(&history, &set))
            .unwrap_err()
            .to_string()
            .contains("pixels per side")
    );
    let anthropic = limits("claude-haiku-5-5");
    let mut set = BTreeMap::new();
    for id in &ids {
        set.insert(id.clone(), fake_prepared(id, 100, 10_000_001));
    }
    assert!(
        preflight(&anthropic, &request(&history, &set))
            .unwrap_err()
            .to_string()
            .contains("bytes per image")
    );
}
