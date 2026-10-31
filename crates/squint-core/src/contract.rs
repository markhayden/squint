//! squint's side of the table-manners contract (ADR 0018, SPEC "External interface"): the
//! contract's objects built from squint's own. The types and schemas are the `table-manners`
//! crate's; this module only fills them in.

use image::DynamicImage;
use image::metadata::Orientation;
use sha2::{Digest, Sha256};
use table_manners::images::{
    BboxPart, ImageReference, Location, LocationKind as ContractKind, Part, PolygonPart,
    Projection as ContractProjection, Source, Topology,
};

use crate::frames::LocationKind;
use crate::image::{CanonicalImage, hex};
use crate::job::Projection;
use crate::report::RootLocation;

/// The contract's reference for a loaded image (table-manners 8): the file's digest and its
/// dimensions as stored, the projection, and the topology that follows from it (squint's
/// panoramas wrap; its flat images are bounded).
pub fn image_reference(image: &CanonicalImage) -> ImageReference {
    let (projection, topology) = match image.projection {
        Projection::Flat => (ContractProjection::Flat, Topology::Bounded),
        Projection::Equirectangular => (ContractProjection::Equirectangular, Topology::Wrap),
    };
    ImageReference {
        file_sha256: image.sha256.clone(),
        width: image.stored_width,
        height: image.stored_height,
        projection,
        topology,
        pixel_sha256: Some(image.pixel_sha256.clone()),
    }
}

/// The contract's pixel digest (table-manners 8.2) of pixels as decoded, before any orientation:
/// 8-bit grey, RGB and RGBA are kept, every other layout becomes 8-bit RGB; SHA-256 over
/// `pixels`, a zero byte, width, height and channel count as little-endian u32, then the samples.
pub fn pixel_sha256(decoded: &DynamicImage) -> String {
    let converted;
    let canonical = match decoded {
        DynamicImage::ImageLuma8(_) | DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgba8(_) => {
            decoded
        }
        other => {
            converted = DynamicImage::ImageRgb8(other.to_rgb8());
            &converted
        }
    };
    let mut hasher = Sha256::new();
    hasher.update(b"pixels");
    hasher.update([0u8]);
    hasher.update(canonical.width().to_le_bytes());
    hasher.update(canonical.height().to_le_bytes());
    hasher.update(u32::from(canonical.color().channel_count()).to_le_bytes());
    hasher.update(canonical.as_bytes());
    hex(&hasher.finalize())
}

/// What a finding says, carried into its contract location.
pub struct FindingWords<'a> {
    /// The finding's reason, in the model's words: the location's `label`, and the `reason` of
    /// an `absent` one.
    pub label: &'a str,
    /// squint's disposition word (`violated`, `allowed`, …).
    pub disposition: &'a str,
    /// squint's confidence word.
    pub confidence: &'a str,
}

/// A finding's location in the contract's form (table-manners 9), from its root location in the
/// raw frame of `image`: mapped back to the file as stored (the inverse of its EXIF orientation),
/// written to a quarter pixel with boxes snapped outward, and split into parts when a footprint
/// crossed a panorama's seam. `None` for a `view`, which never leaves squint as one.
pub fn location(
    image: &CanonicalImage,
    root: &RootLocation,
    words: &FindingWords<'_>,
    run_id: &str,
    item: &str,
) -> Option<Location> {
    let grid = Grid::of(image);
    let (kind, parts, straddles_wrap) = match (root.kind, &root.seam_parts) {
        (LocationKind::Polygon, Some(pieces)) => (
            ContractKind::Polygon,
            pieces.iter().map(|p| grid.polygon(p)).collect(),
            true,
        ),
        (LocationKind::Polygon, None) => (
            ContractKind::Polygon,
            vec![grid.polygon(root.polygon.as_deref()?)],
            false,
        ),
        (LocationKind::Bbox, _) => (ContractKind::Bbox, vec![grid.bbox(root.bbox?)], false),
        (LocationKind::WholeImage, _) => (ContractKind::WholeImage, Vec::new(), false),
        (LocationKind::Absent, _) => (ContractKind::Absent, Vec::new(), false),
        (LocationKind::View, _) => return None,
    };
    Some(Location {
        image: image_reference(image),
        kind,
        parts,
        straddles_wrap,
        label: Some(words.label.to_string()),
        disposition: Some(words.disposition.to_string()),
        confidence: Some(words.confidence.to_string()),
        evidence: None,
        score: None,
        score_kind: None,
        reason: (kind == ContractKind::Absent).then(|| words.label.to_string()),
        source: Some(Source {
            tool: "squint".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            run: Some(run_id.to_string()),
            item: Some(item.to_string()),
        }),
    })
}

/// The stored file's coordinate grid: the inverse of the orientation squint applied, and the
/// precision of a quarter pixel at the longest side.
struct Grid {
    orientation: Orientation,
    scale: f64,
}

impl Grid {
    fn of(image: &CanonicalImage) -> Grid {
        let longest = f64::from(image.stored_width.max(image.stored_height));
        let decimals = (4.0 * longest).log10().ceil();
        Grid {
            orientation: Orientation::from_exif(image.exif_orientation)
                .unwrap_or(Orientation::NoTransforms),
            scale: 10f64.powf(decimals),
        }
    }

    /// A point on the oriented image, as a fraction of the stored file.
    fn stored(&self, [x, y]: [f64; 2]) -> [f64; 2] {
        match self.orientation {
            Orientation::NoTransforms => [x, y],
            Orientation::Rotate90 => [y, 1.0 - x],
            Orientation::Rotate180 => [1.0 - x, 1.0 - y],
            Orientation::Rotate270 => [1.0 - y, x],
            Orientation::FlipHorizontal => [1.0 - x, y],
            Orientation::FlipVertical => [x, 1.0 - y],
            Orientation::Rotate90FlipH => [y, x],
            Orientation::Rotate270FlipH => [1.0 - y, 1.0 - x],
        }
    }

    fn round(&self, v: f64) -> f64 {
        ((v * self.scale).round() / self.scale).clamp(0.0, 1.0)
    }

    /// A box's corners through the orientation, snapped outward so no touched pixel is lost.
    fn bbox(&self, b: [f64; 4]) -> Part {
        let a = self.stored([b[0], b[1]]);
        let c = self.stored([b[2], b[3]]);
        let floor = |v: f64| ((v * self.scale).floor() / self.scale).clamp(0.0, 1.0);
        let ceil = |v: f64| ((v * self.scale).ceil() / self.scale).clamp(0.0, 1.0);
        Part::Bbox(BboxPart {
            bbox: [
                floor(a[0].min(c[0])),
                floor(a[1].min(c[1])),
                ceil(a[0].max(c[0])),
                ceil(a[1].max(c[1])),
            ],
        })
    }

    fn polygon(&self, points: &[[f64; 2]]) -> Part {
        Part::Polygon(PolygonPart {
            polygon: points
                .iter()
                .map(|p| {
                    let q = self.stored(*p);
                    [self.round(q[0]), self.round(q[1])]
                })
                .collect(),
        })
    }
}
