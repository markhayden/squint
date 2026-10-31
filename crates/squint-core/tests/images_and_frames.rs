//! Task 1.2: canonical images (bounds, EXIF, hashes, projection, base64) and the frames registry
//! (location validity, flat mapping, clipping).
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use sha2::Digest;
use squint_core::Error;
use squint_core::frames::{Frame, Location, LocationKind, Mapping, Registry};
use squint_core::image::{self, DecodeBounds};
use squint_core::job::{ImageSource, ImageSpec, Projection, Role};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn spec(id: &str, path: &str) -> ImageSpec {
    ImageSpec {
        id: id.into(),
        role: Role::Candidate,
        label: None,
        source: ImageSource::Path {
            path: path.into(),
            sha256: None,
        },
        projection: None,
        expect: None,
    }
}

fn bounds() -> DecodeBounds {
    DecodeBounds::default()
}

// ---- images --------------------------------------------------------------------------------

/// A path source may pin the file's SHA-256 (evaluation sets do); a mismatch is an input error
/// naming the hash (task 7.2), a match loads, and no pin loads as before.
#[test]
fn a_pinned_sha256_must_match_the_file() {
    let bytes = std::fs::read(root().join("fixtures/images/candidate-64x48.png")).unwrap();
    let actual = image::sha256_hex(&bytes);
    let pinned = |sha: &str| ImageSpec {
        id: "candidate".into(),
        role: Role::Candidate,
        label: None,
        source: ImageSource::Path {
            path: "fixtures/images/candidate-64x48.png".into(),
            sha256: Some(sha.into()),
        },
        projection: None,
        expect: None,
    };
    image::load(&pinned(&actual), &root(), &bounds()).expect("matching pin loads");
    let err = image::load(&pinned(&"0".repeat(64)), &root(), &bounds()).unwrap_err();
    assert!(
        matches!(&err, Error::Input(m) if m.contains("sha256") && m.contains("candidate")),
        "{err}"
    );
}

#[test]
fn oversized_pixel_count_is_refused_before_decode() {
    let err = image::load(
        &spec("big", "fixtures/images/claims-60mp.png"),
        &root(),
        &bounds(),
    )
    .unwrap_err();
    match err {
        Error::Image(m) => assert!(m.contains("60000000") && m.contains("50000000"), "{m}"),
        other => panic!("expected an image error, got {other}"),
    }
}

#[test]
fn oversized_byte_count_is_refused_before_decode() {
    let tiny = DecodeBounds {
        max_pixels: 50_000_000,
        max_bytes: 100,
    };
    let err = image::load(
        &spec("c", "fixtures/images/candidate-64x48.png"),
        &root(),
        &tiny,
    )
    .unwrap_err();
    assert!(matches!(err, Error::Image(m) if m.contains("bytes")));
}

/// Only a regular file is read, and never past the byte bound (review I1): a FIFO or a device is
/// refused without being opened, so a path from an MCP call cannot hang or exhaust the server.
#[test]
fn only_a_regular_file_is_read() {
    let dir = std::env::temp_dir().join(format!("squint-fifo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("image.png");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(made.success());
    for path in [fifo.clone(), dir.clone(), PathBuf::from("/dev/zero")] {
        let spec = spec("c", path.to_str().unwrap());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(image::load(&spec, &root(), &bounds()).map(|_| ()));
        });
        let result = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("{} was opened and blocked", path.display()));
        let err = result.unwrap_err();
        assert!(
            matches!(&err, Error::Image(m) if m.contains("not a regular file")),
            "{}: {err}",
            path.display()
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn exif_orientation_is_applied_and_hashes_are_of_the_original_bytes() {
    let img = image::load(
        &spec("r", "fixtures/images/exif-rotated-64x48.png"),
        &root(),
        &bounds(),
    )
    .unwrap();
    assert_eq!(
        (img.width, img.height),
        (48, 64),
        "orientation 6 rotates 64x48 to 48x64"
    );
    let bytes = std::fs::read(root().join("fixtures/images/exif-rotated-64x48.png")).unwrap();
    let expected = image::hex(&sha2::Sha256::digest(&bytes));
    assert_eq!(img.sha256, expected);
    assert_eq!(img.canonical_sha256.len(), 64);
    assert_ne!(img.canonical_sha256, img.sha256);
}

#[test]
fn canonical_hash_is_the_same_for_identical_pixels_from_different_bytes() {
    let a = image::load(
        &spec("a", "fixtures/images/candidate-64x48.png"),
        &root(),
        &bounds(),
    )
    .unwrap();
    // A base64 copy of the same file decodes to the same pixels.
    let b64 = {
        let bytes = std::fs::read(root().join("fixtures/images/candidate-64x48.png")).unwrap();
        image::base64_encode(&bytes)
    };
    let b_spec = ImageSpec {
        id: "b".into(),
        role: Role::Candidate,
        label: None,
        source: ImageSource::Base64 {
            base64: b64,
            media_type: "image/png".into(),
        },
        projection: None,
        expect: None,
    };
    let b = image::load(&b_spec, &root(), &bounds()).unwrap();
    assert_eq!(a.canonical_sha256, b.canonical_sha256);
    assert_eq!(a.sha256, b.sha256, "same bytes, same original hash");
}

#[test]
fn base64_job_fixture_loads() {
    let job = squint_core::job::Job::load(&root().join("fixtures/jobs/base64.toml")).unwrap();
    let candidate = job.candidate().unwrap();
    let img = image::load(candidate, &root(), &bounds()).unwrap();
    assert_eq!((img.width, img.height), (64, 48));
}

#[test]
fn invalid_base64_is_an_image_error() {
    let s = ImageSpec {
        id: "x".into(),
        role: Role::Candidate,
        label: None,
        source: ImageSource::Base64 {
            base64: "not*base64".into(),
            media_type: "image/png".into(),
        },
        projection: None,
        expect: None,
    };
    assert!(matches!(
        image::load(&s, &root(), &bounds()),
        Err(Error::Image(_))
    ));
}

#[test]
fn gpano_metadata_makes_an_image_equirectangular() {
    let img = image::load(
        &spec("p", "fixtures/images/pano-gpano-128x64.png"),
        &root(),
        &bounds(),
    )
    .unwrap();
    assert_eq!(img.projection, Projection::Equirectangular);
    assert!(img.warnings.is_empty());
}

#[test]
fn two_to_one_without_metadata_is_flat_with_a_warning() {
    let img = image::load(
        &spec("w", "fixtures/images/two-to-one-128x64.png"),
        &root(),
        &bounds(),
    )
    .unwrap();
    assert_eq!(img.projection, Projection::Flat);
    assert_eq!(img.warnings.len(), 1);
    assert!(img.warnings[0].contains("2:1"));
}

#[test]
fn explicit_projection_overrides_detection() {
    let mut s = spec("w", "fixtures/images/two-to-one-128x64.png");
    s.projection = Some(Projection::Equirectangular);
    let img = image::load(&s, &root(), &bounds()).unwrap();
    assert_eq!(img.projection, Projection::Equirectangular);
    assert!(img.warnings.is_empty());
}

// ---- frames ----------------------------------------------------------------------------------

fn registry() -> Registry {
    let mut r = Registry::new();
    r.add_raw("candidate", 64, 48, Projection::Flat);
    r.add_raw("pano", 128, 64, Projection::Equirectangular);
    r
}

fn loc(frame: &str, kind: LocationKind) -> Location {
    Location {
        frame: frame.into(),
        kind,
        bbox: None,
        polygon: None,
        yaw: None,
        pitch: None,
        fov: None,
    }
}

#[test]
fn raw_frames_are_registered_with_root_and_projection() {
    let r = registry();
    let f = r.get("raw:candidate").unwrap();
    assert_eq!(f.root(), "candidate");
    assert_eq!(f.projection(), Projection::Flat);
    assert!(r.get("raw:missing").is_none());
}

#[test]
fn validity_table_bbox() {
    let r = registry();
    let mut l = loc("raw:candidate", LocationKind::Bbox);
    assert!(r.validate(&l).is_err(), "bbox requires bbox");
    l.bbox = Some([0.1, 0.2, 0.3, 0.4]);
    assert!(r.validate(&l).is_ok());
    l.bbox = Some([0.3, 0.2, 0.1, 0.4]);
    assert!(r.validate(&l).is_err(), "x0 < x1");
    l.bbox = Some([0.1, 0.2, 0.3, 0.4]);
    l.yaw = Some(1.0);
    assert!(
        r.validate(&l).is_err(),
        "other coordinate fields must be null"
    );
}

#[test]
fn validity_table_polygon() {
    let r = registry();
    let mut l = loc("raw:candidate", LocationKind::Polygon);
    l.polygon = Some(vec![[0.0, 0.0], [1.0, 0.0]]);
    assert!(r.validate(&l).is_err(), "at least three vertices");
    l.polygon = Some(vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
    assert!(r.validate(&l).is_ok());
    l.polygon = Some(vec![[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]]);
    assert!(r.validate(&l).is_err(), "self-intersecting polygon");
}

#[test]
fn validity_table_view_only_on_equirectangular() {
    let r = registry();
    let mut l = loc("raw:pano", LocationKind::View);
    l.yaw = Some(10.0);
    l.pitch = Some(-5.0);
    l.fov = Some(60.0);
    assert!(r.validate(&l).is_ok());
    l.yaw = Some(180.0);
    assert!(r.validate(&l).is_err(), "yaw upper bound is exclusive");
    l.yaw = Some(10.0);
    l.fov = Some(130.0);
    assert!(r.validate(&l).is_err(), "fov at most 120");
    l.fov = Some(60.0);
    l.frame = "raw:candidate".into();
    assert!(
        r.validate(&l).is_err(),
        "view requires an equirectangular frame"
    );
}

#[test]
fn validity_table_seam_rules_on_equirectangular() {
    let r = registry();
    let mut l = loc("raw:pano", LocationKind::Bbox);
    l.bbox = Some([0.9, 0.2, 1.0, 0.4]);
    assert!(r.validate(&l).is_ok(), "touching the right edge is fine");
    // A normalised box cannot wrap round the seam; one spanning the full width is the full
    // circle (a sky band) and is allowed.
    l.bbox = Some([0.0, 0.2, 1.0, 0.4]);
    assert!(r.validate(&l).is_ok(), "the full width is the full circle");
}

#[test]
fn validity_table_whole_image_and_absent() {
    let r = registry();
    let mut w = loc("raw:candidate", LocationKind::WholeImage);
    assert!(r.validate(&w).is_ok());
    w.bbox = Some([0.0, 0.0, 1.0, 1.0]);
    assert!(
        r.validate(&w).is_err(),
        "whole_image carries no coordinates"
    );
    let a = loc("raw:candidate", LocationKind::Absent);
    assert!(r.validate(&a).is_ok());
    let bad = loc("raw:nope", LocationKind::Absent);
    assert!(r.validate(&bad).is_err(), "frame must exist");
}

#[test]
fn crop_frame_maps_back_to_raw_and_round_trips() {
    let mut r = registry();
    r.add_crop(
        "crop:call:1:0",
        "raw:candidate",
        Mapping::Bbox([0.25, 0.25, 0.75, 0.75]),
        32,
        24,
    );
    let mut l = loc("crop:call:1:0", LocationKind::Bbox);
    l.bbox = Some([0.0, 0.0, 0.5, 0.5]);
    let mapped = r.to_root(&l).unwrap();
    assert_eq!(mapped.frame, "raw:candidate");
    let b = mapped.bbox.unwrap();
    for (got, want) in b.iter().zip([0.25, 0.25, 0.5, 0.5]) {
        assert!((got - want).abs() < 1e-6, "{b:?}");
    }
    assert!(!mapped.partial);
    // Map a raw location into the crop and back.
    let back = r.from_root("crop:call:1:0", &mapped.location).unwrap();
    for (got, want) in back.bbox.unwrap().iter().zip([0.0, 0.0, 0.5, 0.5]) {
        assert!((got - want).abs() < 1e-6);
    }
}

#[test]
fn mapping_partly_outside_the_parent_is_clipped_and_flagged() {
    let mut r = registry();
    // A crop whose own mapping extends past the right edge of its parent.
    r.add_crop(
        "crop:call:2:0",
        "raw:candidate",
        Mapping::Bbox([0.75, 0.0, 1.25, 0.5]),
        32,
        24,
    );
    let mut l = loc("crop:call:2:0", LocationKind::Bbox);
    l.bbox = Some([0.0, 0.0, 1.0, 1.0]);
    let mapped = r.to_root(&l).unwrap();
    assert!(mapped.partial);
    let b = mapped.bbox.unwrap();
    assert!(
        (b[2] - 1.0).abs() < 1e-9,
        "clipped to the parent's right edge, got {b:?}"
    );
}

#[test]
fn mapping_wholly_outside_the_parent_is_an_error() {
    let mut r = registry();
    r.add_crop(
        "crop:call:3:0",
        "raw:candidate",
        Mapping::Bbox([1.5, 0.0, 2.0, 0.5]),
        32,
        24,
    );
    let mut l = loc("crop:call:3:0", LocationKind::Bbox);
    l.bbox = Some([0.0, 0.0, 1.0, 1.0]);
    assert!(r.to_root(&l).is_err());
}

#[test]
fn frames_have_kinds() {
    let mut r = registry();
    r.add_crop(
        "crop:call:1:0",
        "raw:candidate",
        Mapping::Bbox([0.0, 0.0, 0.5, 0.5]),
        32,
        24,
    );
    assert!(matches!(
        r.get("raw:candidate").unwrap().frame(),
        Frame::Raw { .. }
    ));
    assert!(matches!(
        r.get("crop:call:1:0").unwrap().frame(),
        Frame::Crop { .. }
    ));
}
