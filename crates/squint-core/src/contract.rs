//! squint's side of the table-manners contract (ADR 0018, SPEC "External interface"): the
//! contract's objects built from squint's own. The types and schemas are the `table-manners`
//! crate's; this module only fills them in.

use image::DynamicImage;
use sha2::{Digest, Sha256};
use table_manners::images::{ImageReference, Projection as ContractProjection, Topology};

use crate::image::{CanonicalImage, hex};
use crate::job::Projection;

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
