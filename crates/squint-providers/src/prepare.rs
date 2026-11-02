//! Image preparation and preflight (SPEC "Image preparation"). Preparation is deterministic and
//! versioned: the canonical pixels are resized with Lanczos3 so the long edge fits the model's
//! native size (never upscaled), encoded as JPEG at a pinned quality, or as PNG when the image has
//! an alpha channel, with no metadata, and hashed. Preflight runs over a whole outgoing request,
//! counting every image in the history including tool results, and refuses a request that would
//! exceed any catalog limit with `error: image_limit`; nothing is ever downscaled silently. When a
//! conversation can pass the many-image threshold, resolve plans every image at the many-image
//! size from the start ([`ImagePlan::Many`]), so preflight never meets it mid-run.

use std::io::Cursor;

use squint_core::image::{CanonicalImage, ImagePlan, Prepared, fit, hex};
use squint_core::provider::{ContentBlock, TurnRequest};

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{DynamicImage, ImageEncoder, imageops};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use squint_core::{Error, Result};

use crate::catalog::Images;

/// The preparation version: Lanczos3 fit to the native long edge, JPEG quality 90 or PNG with
/// alpha, metadata stripped. Bump when any of that changes; it is part of the cache key.
pub const PREP_VERSION: &str = "1";
pub const JPEG_QUALITY: u8 = 90;

/// Prepares images for one model under one decision's plan.
pub struct Preparer<'a> {
    images: &'a Images,
    long_edge: u32,
    prep_version: String,
}

impl<'a> Preparer<'a> {
    /// `max_long_edge` is the config's `images.max_long_edge`, already checked against the native
    /// size by the catalog; the smaller of the two wins, and a many-image plan's edge lowers it
    /// further. The plan is recorded in every prepared image's `prep_version`.
    pub fn new(images: &'a Images, max_long_edge: Option<u32>, plan: ImagePlan) -> Preparer<'a> {
        let long_edge = [max_long_edge, plan.long_edge()]
            .into_iter()
            .flatten()
            .fold(images.native_long_edge_px, u32::min)
            .max(1);
        Preparer {
            images,
            long_edge,
            prep_version: plan.prep_version(PREP_VERSION),
        }
    }

    /// The long edge prepared images are fitted to.
    pub fn long_edge(&self) -> u32 {
        self.long_edge
    }

    /// A job image, keyed by its id.
    pub fn prepare_canonical(&self, img: &CanonicalImage) -> Result<Prepared> {
        self.prepare(&img.id, &img.dynamic()?)
    }

    /// Any pixels (a tool's returned frame, decoded by the caller), keyed by the given id.
    pub fn prepare(&self, id: &str, pixels: &DynamicImage) -> Result<Prepared> {
        let (sw, sh) = (pixels.width(), pixels.height());
        if sw == 0 || sh == 0 {
            return Err(Error::Image(format!("{id}: empty image")));
        }
        let (w, h) = fit(sw, sh, self.long_edge);
        let has_alpha = pixels.color().has_alpha();
        let resized: DynamicImage = if (w, h) == (sw, sh) {
            if has_alpha {
                DynamicImage::ImageRgba8(pixels.to_rgba8())
            } else {
                DynamicImage::ImageRgb8(pixels.to_rgb8())
            }
        } else if has_alpha {
            DynamicImage::ImageRgba8(imageops::resize(
                &pixels.to_rgba8(),
                w,
                h,
                imageops::FilterType::Lanczos3,
            ))
        } else {
            DynamicImage::ImageRgb8(imageops::resize(
                &pixels.to_rgb8(),
                w,
                h,
                imageops::FilterType::Lanczos3,
            ))
        };
        let mut bytes = Vec::new();
        let media_type = if has_alpha {
            PngEncoder::new_with_quality(
                Cursor::new(&mut bytes),
                CompressionType::Default,
                FilterType::Adaptive,
            )
            .write_image(resized.as_bytes(), w, h, image::ExtendedColorType::Rgba8)
            .map_err(|e| Error::Image(format!("{id}: cannot encode PNG: {e}")))?;
            "image/png"
        } else {
            JpegEncoder::new_with_quality(Cursor::new(&mut bytes), JPEG_QUALITY)
                .write_image(resized.as_bytes(), w, h, image::ExtendedColorType::Rgb8)
                .map_err(|e| Error::Image(format!("{id}: cannot encode JPEG: {e}")))?;
            "image/jpeg"
        };
        if !self.images.formats.iter().any(|f| f == media_type) {
            return Err(Error::ImageLimit(format!(
                "{id}: the model accepts {:?}, not {media_type}",
                self.images.formats
            )));
        }
        Ok(Prepared {
            id: id.to_string(),
            media_type: media_type.to_string(),
            width: w,
            height: h,
            source_width: sw,
            source_height: sh,
            sha256: hex(&Sha256::digest(&bytes)),
            prep_version: self.prep_version.clone(),
            bytes,
        })
    }
}

/// What a request carries, as preflight measured it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preflight {
    pub images: u64,
    pub image_bytes: u64,
    /// Text plus base64-encoded images, the size the provider will see.
    pub request_bytes: u64,
}

/// Every image in the outgoing history, including tool results, against the model's limits; the
/// bytes come from the request's prepared images.
pub fn preflight(limits: &Images, request: &TurnRequest<'_>) -> Result<Preflight> {
    let mut ids: Vec<&str> = Vec::new();
    let mut text_bytes = request.system.len() as u64;
    for message in request.history {
        for block in &message.content {
            match block {
                ContentBlock::Text { text } => text_bytes += text.len() as u64,
                ContentBlock::Image { image_id } => ids.push(image_id),
                ContentBlock::ToolUse { arguments, .. } => {
                    text_bytes += arguments.to_string().len() as u64
                }
                ContentBlock::ToolResult {
                    content, images, ..
                } => {
                    text_bytes += content.len() as u64;
                    ids.extend(images.iter().map(String::as_str));
                }
                ContentBlock::Opaque { block, .. } => text_bytes += block.to_string().len() as u64,
            }
        }
    }
    for tool in request.tools {
        text_bytes += tool.input_schema.to_string().len() as u64 + tool.description.len() as u64;
    }
    let count = ids.len() as u64;
    if count > u64::from(limits.max_per_request) {
        return Err(Error::ImageLimit(format!(
            "{count} images in the request exceed the model's limit of {} per request",
            limits.max_per_request
        )));
    }
    let many = limits
        .many_image_threshold
        .zip(limits.many_image_max_dimension_px)
        .filter(|(threshold, _)| count > u64::from(*threshold));
    let mut image_bytes = 0u64;
    for id in ids {
        let img = request.images.get(id).ok_or_else(|| {
            Error::Image(format!(
                "{id}: referenced by the request but never prepared"
            ))
        })?;
        if !limits.formats.contains(&img.media_type) {
            return Err(Error::ImageLimit(format!(
                "{id}: {} is not among the model's accepted formats {:?}",
                img.media_type, limits.formats
            )));
        }
        let bytes = img.bytes.len() as u64;
        if bytes > limits.max_bytes {
            return Err(Error::ImageLimit(format!(
                "{id}: {bytes} bytes exceed the model's limit of {} bytes per image",
                limits.max_bytes
            )));
        }
        let long = img.width.max(img.height);
        if let Some(max) = limits.max_dimension_px
            && long > max
        {
            return Err(Error::ImageLimit(format!(
                "{id}: {}x{} exceeds the model's limit of {max} pixels per side",
                img.width, img.height
            )));
        }
        if let Some((threshold, max)) = many
            && long > max
        {
            return Err(Error::ImageLimit(format!(
                "{id}: {}x{} exceeds {max} pixels per side, the limit when a request carries more than {threshold} images ({count} here)",
                img.width, img.height
            )));
        }
        image_bytes += bytes;
    }
    // Base64 grows bytes by a third; the framing per image is small and counted generously.
    let request_bytes = text_bytes + image_bytes.div_ceil(3) * 4 + count * 256;
    if request_bytes > limits.max_request_bytes {
        return Err(Error::ImageLimit(format!(
            "the request would be about {request_bytes} bytes, over the model's limit of {} bytes",
            limits.max_request_bytes
        )));
    }
    Ok(Preflight {
        images: count,
        image_bytes,
        request_bytes,
    })
}
