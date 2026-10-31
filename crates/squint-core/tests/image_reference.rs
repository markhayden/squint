//! Task 7.2: the contract's image reference (table-manners 8) for every job image, and the
//! `expect` reference a job may carry, verified before anything else runs.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::Error;
use squint_core::contract;
use squint_core::image::{self, DecodeBounds};
use squint_core::job::{ImageSource, ImageSpec, Projection, Role};
use table_manners::images::{ImageReference, Projection as Proj, Topology};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn spec(path: &str, expect: Option<ImageReference>) -> ImageSpec {
    ImageSpec {
        id: "candidate".into(),
        role: Role::Candidate,
        label: None,
        source: ImageSource::Path {
            path: path.into(),
            sha256: None,
        },
        projection: None,
        expect,
    }
}

fn load(spec: &ImageSpec) -> squint_core::Result<image::CanonicalImage> {
    image::load(spec, &root(), &DecodeBounds::default())
}

fn reference_of(path: &str) -> ImageReference {
    contract::image_reference(&load(&spec(path, None)).unwrap())
}

#[test]
fn every_contract_pixel_vector_reproduces_through_squints_decode() {
    let dir = root().join("fixtures/contract/pixels");
    let digests: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("digests.json")).unwrap()).unwrap();
    let vectors = digests["vectors"].as_array().unwrap();
    assert_eq!(vectors.len(), 12);
    for v in vectors {
        let file = v["file"].as_str().unwrap();
        let bytes = std::fs::read(dir.join(file)).unwrap();
        assert_eq!(
            image::sha256_hex(&bytes),
            v["file_sha256"],
            "{file}: the copy differs from the contract's vector"
        );
        let reference = reference_of(&format!("fixtures/contract/pixels/{file}"));
        assert_eq!(reference.file_sha256, v["file_sha256"], "{file}");
        assert_eq!(u64::from(reference.width), v["width"], "{file}: width");
        assert_eq!(u64::from(reference.height), v["height"], "{file}: height");
        assert_eq!(
            reference.pixel_sha256.as_deref(),
            v["pixel_sha256"].as_str(),
            "{file}: pixel_sha256"
        );
    }
}

/// The reference describes the file as stored: an EXIF rotation swaps squint's canonical
/// dimensions, never the reference's.
#[test]
fn the_reference_has_the_stored_dimensions_not_the_oriented_ones() {
    let img = load(&spec("fixtures/images/exif-rotated-64x48.png", None)).unwrap();
    assert_eq!((img.width, img.height), (48, 64));
    let reference = contract::image_reference(&img);
    assert_eq!((reference.width, reference.height), (64, 48));
    assert_eq!(reference.file_sha256, img.sha256);
}

#[test]
fn flat_images_are_bounded_and_panoramas_wrap() {
    let flat = reference_of("fixtures/images/candidate-64x48.png");
    assert_eq!(
        (flat.projection, flat.topology),
        (Proj::Flat, Topology::Bounded)
    );
    let mut pano = spec("fixtures/images/candidate-64x48.png", None);
    pano.projection = Some(Projection::Equirectangular);
    let pano = contract::image_reference(&load(&pano).unwrap());
    assert_eq!(
        (pano.projection, pano.topology),
        (Proj::Equirectangular, Topology::Wrap)
    );
}

#[test]
fn a_matching_expect_loads() {
    let expect = reference_of("fixtures/images/candidate-64x48.png");
    load(&spec("fixtures/images/candidate-64x48.png", Some(expect))).unwrap();
}

#[test]
fn an_expect_with_another_digest_is_refused_as_input() {
    let mut expect = reference_of("fixtures/images/candidate-64x48.png");
    expect.file_sha256 = "0".repeat(64);
    let err = load(&spec("fixtures/images/candidate-64x48.png", Some(expect))).unwrap_err();
    assert!(
        matches!(&err, Error::Input(m) if m.contains("candidate") && m.contains("file_sha256")),
        "{err}"
    );
    assert_eq!(err.class(), "input");
}

#[test]
fn an_expect_with_other_dimensions_is_refused_as_input() {
    let mut expect = reference_of("fixtures/images/candidate-64x48.png");
    expect.width += 1;
    let err = load(&spec("fixtures/images/candidate-64x48.png", Some(expect))).unwrap_err();
    assert!(
        matches!(&err, Error::Input(m) if m.contains("width")),
        "{err}"
    );
}

/// Decoders may disagree on lossy formats, so a different pixel digest with the right file is a
/// note, never a refusal (table-manners 8.2).
#[test]
fn an_expect_with_another_pixel_digest_but_the_same_file_loads_with_a_note() {
    let mut expect = reference_of("fixtures/images/candidate-64x48.png");
    expect.pixel_sha256 = Some("f".repeat(64));
    let img = load(&spec("fixtures/images/candidate-64x48.png", Some(expect))).unwrap();
    assert!(
        img.warnings.iter().any(|w| w.contains("pixel_sha256")),
        "{:?}",
        img.warnings
    );
}

/// A pinned SHA-256 (evaluation-set jobs) that does not match is the same refusal as an expect.
#[test]
fn a_pinned_sha256_that_does_not_match_is_input() {
    let mut pinned = spec("fixtures/images/candidate-64x48.png", None);
    pinned.source = ImageSource::Path {
        path: "fixtures/images/candidate-64x48.png".into(),
        sha256: Some("0".repeat(64)),
    };
    assert_eq!(load(&pinned).unwrap_err().class(), "input");
}
