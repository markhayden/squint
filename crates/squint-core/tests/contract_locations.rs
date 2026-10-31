//! Task 7.3: findings leave squint as the contract's locations (table-manners 9): on the file as
//! stored, at a quarter pixel, split at a panorama's seam, and valid against the contract's schema.
#![allow(clippy::unwrap_used)]

use image::metadata::Orientation;
use image::{DynamicImage, GenericImageView, Rgb, RgbImage};
use squint_core::contract::{self, FindingWords};
use squint_core::frames::{Location, LocationKind, Mapping, Registry};
use squint_core::image::CanonicalImage;
use squint_core::job::{Projection, Role};
use squint_core::report::RootLocation;
use table_manners::images::{LocationKind as ContractKind, Part};

fn canonical(
    stored: (u32, u32),
    orientation: Orientation,
    projection: Projection,
) -> CanonicalImage {
    let (width, height) = match orientation {
        Orientation::Rotate90
        | Orientation::Rotate270
        | Orientation::Rotate90FlipH
        | Orientation::Rotate270FlipH => (stored.1, stored.0),
        _ => stored,
    };
    CanonicalImage {
        id: "candidate".into(),
        role: Role::Candidate,
        label: None,
        width,
        height,
        stored_width: stored.0,
        stored_height: stored.1,
        exif_orientation: orientation.to_exif(),
        sha256: "a".repeat(64),
        canonical_sha256: "b".repeat(64),
        pixel_sha256: "c".repeat(64),
        projection,
        has_icc_profile: false,
        has_alpha: false,
        warnings: Vec::new(),
        pixels: None,
        alpha: None,
    }
}

fn root(kind: LocationKind, bbox: Option<[f64; 4]>) -> RootLocation {
    RootLocation {
        image: "candidate".into(),
        frame: "raw:candidate".into(),
        kind,
        bbox,
        polygon: None,
        partial: false,
        seam_parts: None,
    }
}

const WORDS: FindingWords<'static> = FindingWords {
    label: "a gull on the left lamp post",
    disposition: "violated",
    confidence: "confident",
};

fn emit(image: &CanonicalImage, root: &RootLocation) -> table_manners::images::Location {
    let loc =
        contract::location(image, root, &WORDS, "decision-1", "chk:ob:SF1@candidate").unwrap();
    valid(&loc);
    loc
}

/// Every emitted location validates against the contract's location schema.
fn valid(loc: &table_manners::images::Location) {
    struct Embedded;
    impl jsonschema::Retrieve for Embedded {
        fn retrieve(
            &self,
            uri: &jsonschema::Uri<String>,
        ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
            let (_, text) = table_manners::SCHEMAS
                .iter()
                .find(|(id, _)| *id == uri.as_str())
                .ok_or("unknown schema")?;
            Ok(serde_json::from_str(text)?)
        }
    }
    let (_, text) = table_manners::SCHEMAS
        .iter()
        .find(|(id, _)| *id == "urn:table-manners:images:location")
        .unwrap();
    let schema: serde_json::Value = serde_json::from_str(text).unwrap();
    let validator = jsonschema::options()
        .with_retriever(Embedded)
        .build(&schema)
        .unwrap();
    let doc = serde_json::to_value(loc).unwrap();
    let errors: Vec<String> = validator.iter_errors(&doc).map(|e| e.to_string()).collect();
    assert!(errors.is_empty(), "{errors:?}\n{doc}");
}

fn bbox_of(loc: &table_manners::images::Location) -> [f64; 4] {
    match &loc.parts[..] {
        [Part::Bbox(b)] => b.bbox,
        other => panic!("one bbox part expected: {other:?}"),
    }
}

/// A box on one displayed pixel lands, through the inverse of each EXIF orientation, on the
/// stored pixel that shows there.
#[test]
fn locations_are_mapped_back_through_every_exif_orientation() {
    let (w, h) = (7u32, 5u32);
    let stored =
        DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| Rgb([x as u8, y as u8, 9])));
    for exif in 1..=8u8 {
        let orientation = Orientation::from_exif(exif).unwrap();
        let mut shown = stored.clone();
        shown.apply_orientation(orientation);
        let image = canonical((w, h), orientation, Projection::Flat);
        assert_eq!((image.width, image.height), shown.dimensions());
        for (x, y) in [
            (0, 0),
            (shown.width() - 1, 0),
            (1, shown.height() - 1),
            (2, 3),
        ] {
            let (sw, sh) = (f64::from(shown.width()), f64::from(shown.height()));
            let pixel = [
                f64::from(x) / sw,
                f64::from(y) / sh,
                f64::from(x + 1) / sw,
                f64::from(y + 1) / sh,
            ];
            let b = bbox_of(&emit(&image, &root(LocationKind::Bbox, Some(pixel))));
            let centre = (
                ((b[0] + b[2]) / 2.0 * f64::from(w)) as u32,
                ((b[1] + b[3]) / 2.0 * f64::from(h)) as u32,
            );
            assert_eq!(
                stored.get_pixel(centre.0, centre.1),
                shown.get_pixel(x, y),
                "orientation {exif}, shown pixel ({x}, {y})"
            );
        }
    }
}

/// Coordinates are written to a quarter pixel at the longest side, and a box is snapped outward.
#[test]
fn coordinates_are_written_to_a_quarter_pixel_and_boxes_snap_outward() {
    let image = canonical((2048, 1024), Orientation::NoTransforms, Projection::Flat);
    let b = bbox_of(&emit(
        &image,
        &root(
            LocationKind::Bbox,
            Some([0.123_456_7, 0.2, 0.765_432_1, 0.8]),
        ),
    ));
    // 4 × 2048 = 8192 needs four decimals.
    for v in b {
        assert!(((v * 1e4).round() - v * 1e4).abs() < 1e-6, "{v}");
    }
    assert!(b[0] <= 0.123_456_7 && b[2] >= 0.765_432_1, "{b:?}");
}

/// A view footprint across a panorama's seam leaves as two parts, one each side of the wrap edge.
#[test]
fn a_footprint_across_the_seam_leaves_as_two_parts() {
    let mut registry = Registry::new();
    registry.add_raw("pano", 2048, 1024, Projection::Equirectangular);
    registry.add_crop(
        "crop:call:1:0",
        "raw:pano",
        Mapping::View {
            yaw: 178.0,
            pitch: 0.0,
            fov: 60.0,
        },
        512,
        512,
    );
    let mapped = registry
        .to_root(&Location {
            frame: "crop:call:1:0".into(),
            kind: LocationKind::Bbox,
            bbox: Some([0.2, 0.3, 0.8, 0.7]),
            polygon: None,
            yaw: None,
            pitch: None,
            fov: None,
        })
        .unwrap();
    let parts = mapped
        .seam_parts
        .clone()
        .expect("the footprint crosses the seam");
    let root = RootLocation {
        image: "pano".into(),
        frame: mapped.location.frame.clone(),
        kind: mapped.location.kind,
        bbox: mapped.location.bbox,
        polygon: mapped.location.polygon.clone(),
        partial: mapped.partial,
        seam_parts: Some(parts),
    };
    let image = canonical(
        (2048, 1024),
        Orientation::NoTransforms,
        Projection::Equirectangular,
    );
    let loc = emit(&image, &root);
    assert_eq!(loc.kind, ContractKind::Polygon);
    assert!(loc.straddles_wrap);
    let xs: Vec<(f64, f64)> = loc
        .parts
        .iter()
        .map(|p| match p {
            Part::Polygon(p) => p.polygon.iter().fold((1.0, 0.0), |(lo, hi), q| {
                (f64::min(lo, q[0]), f64::max(hi, q[0]))
            }),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(xs.len(), 2);
    assert!(
        xs.iter().any(|(lo, _)| *lo == 0.0) && xs.iter().any(|(_, hi)| *hi == 1.0),
        "one part touches each edge: {xs:?}"
    );
}

/// A footprint inside the panorama, and anything on a flat image, is one part that never straddles.
#[test]
fn a_footprint_away_from_the_seam_is_one_part() {
    let mut registry = Registry::new();
    registry.add_raw("pano", 2048, 1024, Projection::Equirectangular);
    registry.add_crop(
        "crop:call:1:0",
        "raw:pano",
        Mapping::View {
            yaw: 30.0,
            pitch: 0.0,
            fov: 60.0,
        },
        512,
        512,
    );
    let mapped = registry
        .to_root(&Location {
            frame: "crop:call:1:0".into(),
            kind: LocationKind::Bbox,
            bbox: Some([0.2, 0.3, 0.8, 0.7]),
            polygon: None,
            yaw: None,
            pitch: None,
            fov: None,
        })
        .unwrap();
    assert!(mapped.seam_parts.is_none());
    let mut r = root(LocationKind::Polygon, None);
    r.polygon = mapped.location.polygon.clone();
    let image = canonical(
        (2048, 1024),
        Orientation::NoTransforms,
        Projection::Equirectangular,
    );
    let loc = emit(&image, &r);
    assert_eq!((loc.parts.len(), loc.straddles_wrap), (1, false));
    let flat = canonical((640, 480), Orientation::NoTransforms, Projection::Flat);
    let loc = emit(&flat, &root(LocationKind::Bbox, Some([0.1, 0.1, 0.2, 0.2])));
    assert!(!loc.straddles_wrap);
}

#[test]
fn absent_and_whole_image_carry_no_geometry_and_absent_has_its_reason() {
    let image = canonical((640, 480), Orientation::NoTransforms, Projection::Flat);
    let absent = emit(&image, &root(LocationKind::Absent, None));
    assert_eq!(absent.kind, ContractKind::Absent);
    assert!(absent.parts.is_empty());
    assert_eq!(absent.reason.as_deref(), Some(WORDS.label));
    let whole = emit(&image, &root(LocationKind::WholeImage, None));
    assert_eq!(whole.kind, ContractKind::WholeImage);
    assert!(whole.parts.is_empty() && whole.reason.is_none());
}

#[test]
fn the_source_names_squint_its_version_the_run_and_the_item() {
    let image = canonical((640, 480), Orientation::NoTransforms, Projection::Flat);
    let loc = emit(&image, &root(LocationKind::WholeImage, None));
    let source = loc.source.unwrap();
    assert_eq!(source.tool, "squint");
    assert_eq!(source.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(source.run.as_deref(), Some("decision-1"));
    assert_eq!(source.item.as_deref(), Some("chk:ob:SF1@candidate"));
    assert_eq!(loc.label.as_deref(), Some(WORDS.label));
    assert_eq!(loc.image, contract::image_reference(&image));
}
