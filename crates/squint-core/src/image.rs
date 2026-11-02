//! Canonical images: the decoded, identified form of a job image. Decode bounds are checked from
//! the header before pixels are decoded; EXIF orientation is applied; pixels are kept as 8-bit RGB
//! (an embedded ICC profile is recorded but not applied: the `image` crate has no colour
//! management, so "sRGB" here means 8-bit RGB as decoded); the original bytes and the canonical
//! pixels are both hashed; projection comes from the spec, else GPano metadata, else `flat`.

use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::Arc;

use image::{DynamicImage, ImageDecoder, ImageReader, metadata::Orientation};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::job::{ImageSource, ImageSpec, Projection, Role};
use crate::{Error, Result};

/// Limits applied before decoding. Config `[images]` overrides them (task 1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecodeBounds {
    pub max_pixels: u64,
    pub max_bytes: u64,
}

impl Default for DecodeBounds {
    fn default() -> Self {
        DecodeBounds {
            max_pixels: 50_000_000,
            max_bytes: 64 * 1024 * 1024,
        }
    }
}

/// A loaded job image.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalImage {
    pub id: String,
    pub role: Role,
    pub label: Option<String>,
    /// Dimensions after orientation normalisation.
    pub width: u32,
    pub height: u32,
    /// Dimensions as stored in the file, before EXIF orientation: the contract reference's.
    pub stored_width: u32,
    pub stored_height: u32,
    /// The EXIF orientation applied to the stored pixels (1 to 8; 1 is none), so locations can be
    /// mapped back to the file as stored.
    pub exif_orientation: u8,
    /// SHA-256 of the original bytes, lowercase hex.
    pub sha256: String,
    /// SHA-256 of the canonical pixels (8-bit RGB, row-major, after orientation), lowercase hex.
    pub canonical_sha256: String,
    /// The contract's pixel digest (table-manners 8.2): the stored pixels, before orientation.
    pub pixel_sha256: String,
    pub projection: Projection,
    /// Whether the file carried an ICC profile (recorded, not applied).
    pub has_icc_profile: bool,
    /// Whether the file carried an alpha channel; the plane is kept in `alpha` for preparation.
    pub has_alpha: bool,
    /// Non-fatal notes, for example the 2:1 aspect ratio treated as flat.
    pub warnings: Vec<String>,
    /// The canonical pixels, shared with tools; not serialised.
    #[serde(skip)]
    #[schemars(skip)]
    pub pixels: Option<Arc<image::RgbImage>>,
    /// The alpha plane when the file had one (same dimensions as `pixels`); not serialised.
    #[serde(skip)]
    #[schemars(skip)]
    pub alpha: Option<Arc<image::GrayImage>>,
}

impl CanonicalImage {
    /// The pixels as one image: RGBA when the file had an alpha channel, RGB otherwise.
    pub fn dynamic(&self) -> Result<DynamicImage> {
        let pixels = self
            .pixels
            .as_ref()
            .ok_or_else(|| Error::Image(format!("{}: no pixels", self.id)))?;
        match &self.alpha {
            Some(alpha) if alpha.dimensions() == pixels.dimensions() => {
                let mut rgba = image::RgbaImage::new(pixels.width(), pixels.height());
                for (x, y, p) in rgba.enumerate_pixels_mut() {
                    let rgb = pixels.get_pixel(x, y).0;
                    *p = image::Rgba([rgb[0], rgb[1], rgb[2], alpha.get_pixel(x, y).0[0]]);
                }
                Ok(DynamicImage::ImageRgba8(rgba))
            }
            _ => Ok(DynamicImage::ImageRgb8(pixels.as_ref().clone())),
        }
    }
}

/// One image as a provider receives it (SPEC "Image preparation"): the bytes, their hash and the
/// preparation that produced them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prepared {
    /// The id the request's content blocks use (a job image id or a frame id).
    pub id: String,
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub source_width: u32,
    pub source_height: u32,
    /// SHA-256 of the prepared bytes, lowercase hex.
    pub sha256: String,
    pub prep_version: String,
    /// The bytes sent; stored as an artifact by the harness, never serialised here.
    #[serde(skip)]
    pub bytes: Vec<u8>,
}

impl Prepared {
    pub fn resized(&self) -> bool {
        (self.width, self.height) != (self.source_width, self.source_height)
    }
}

/// The canonical pixels as lossless PNG, unresized: what a provider without its own preparation
/// sends (`prep_version` `canonical`).
pub fn canonical_png(id: &str, pixels: &DynamicImage) -> Result<Prepared> {
    use image::ImageEncoder;
    let (w, h) = (pixels.width(), pixels.height());
    let mut bytes = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new_with_quality(
        Cursor::new(&mut bytes),
        image::codecs::png::CompressionType::Default,
        image::codecs::png::FilterType::Adaptive,
    );
    let result = if pixels.color().has_alpha() {
        encoder.write_image(
            pixels.to_rgba8().as_raw(),
            w,
            h,
            image::ExtendedColorType::Rgba8,
        )
    } else {
        encoder.write_image(
            pixels.to_rgb8().as_raw(),
            w,
            h,
            image::ExtendedColorType::Rgb8,
        )
    };
    result.map_err(|e| Error::Image(format!("{id}: cannot encode PNG: {e}")))?;
    Ok(Prepared {
        id: id.to_string(),
        media_type: "image/png".into(),
        width: w,
        height: h,
        source_width: w,
        source_height: h,
        sha256: hex(&Sha256::digest(&bytes)),
        prep_version: "canonical".into(),
        bytes,
    })
}

/// Loads one image spec, resolving a relative path against `base`.
pub fn load(spec: &ImageSpec, base: &Path, bounds: &DecodeBounds) -> Result<CanonicalImage> {
    let bytes = read_bytes(spec, base, bounds.max_bytes)?;
    if bytes.len() as u64 > bounds.max_bytes {
        return Err(Error::ImageLimit(format!(
            "{}: {} bytes exceeds the decode bound of {} bytes",
            spec.id,
            bytes.len(),
            bounds.max_bytes
        )));
    }
    let reader = ImageReader::new(Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| Error::Image(format!("{}: cannot read: {e}", spec.id)))?;
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| Error::Image(format!("{}: cannot decode header: {e}", spec.id)))?;
    let (w, h) = decoder.dimensions();
    if let Some(expect) = &spec.expect
        && (expect.width, expect.height) != (w, h)
    {
        return Err(Error::Input(format!(
            "{}: the file is {w}x{h} as stored; the expected reference has width {} and height {}",
            spec.id, expect.width, expect.height
        )));
    }
    let pixels = u64::from(w) * u64::from(h);
    if pixels > bounds.max_pixels {
        return Err(Error::ImageLimit(format!(
            "{}: {w}x{h} = {pixels} pixels exceeds the decode bound of {} pixels",
            spec.id, bounds.max_pixels
        )));
    }
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let has_icc_profile = decoder.icc_profile().ok().flatten().is_some();
    let mut dynamic = DynamicImage::from_decoder(decoder)
        .map_err(|e| Error::Image(format!("{}: cannot decode: {e}", spec.id)))?;
    let pixel_sha256 = crate::contract::pixel_sha256(&dynamic);
    dynamic.apply_orientation(orientation);
    let has_alpha = dynamic.color().has_alpha();
    let alpha = has_alpha.then(|| {
        let rgba = dynamic.to_rgba8();
        let plane: Vec<u8> = rgba.pixels().map(|p| p.0[3]).collect();
        image::GrayImage::from_raw(rgba.width(), rgba.height(), plane)
    });
    let rgb = dynamic.to_rgb8();

    let mut warnings = Vec::new();
    if let Some(expected) = spec.expect.as_ref().and_then(|e| e.pixel_sha256.as_deref())
        && expected != pixel_sha256
    {
        warnings.push(format!(
            "{}: pixel_sha256 {pixel_sha256} differs from the expected {expected}; the file \
             matches, so decoders disagree on its pixels",
            spec.id
        ));
    }
    let projection = match spec.projection {
        Some(p) => p,
        None if has_gpano_equirectangular(&bytes) => Projection::Equirectangular,
        None => {
            if rgb.width() == 2 * rgb.height() {
                warnings.push(format!(
                    "{}: 2:1 aspect ratio but no projection metadata; treated as flat. Set \
                     projection = \"equirectangular\" if it is a panorama.",
                    spec.id
                ));
            }
            Projection::Flat
        }
    };

    Ok(CanonicalImage {
        id: spec.id.clone(),
        role: spec.role,
        label: spec.label.clone(),
        width: rgb.width(),
        height: rgb.height(),
        stored_width: w,
        stored_height: h,
        exif_orientation: orientation.to_exif(),
        sha256: hex(&Sha256::digest(&bytes)),
        canonical_sha256: hex(&Sha256::digest(rgb.as_raw())),
        pixel_sha256,
        projection,
        has_icc_profile,
        has_alpha,
        warnings,
        pixels: Some(Arc::new(rgb)),
        alpha: alpha.flatten().map(Arc::new),
    })
}

/// The image's file digest (table-manners 8.1) without decoding it: the bytes read as [`load`]
/// reads them, bounded by `max_bytes`, so it equals the loaded image's `sha256`. What a server
/// recomputes a call's estimate binding from.
pub fn file_sha256(spec: &ImageSpec, base: &Path, max_bytes: u64) -> Result<String> {
    let bytes = read_bytes(spec, base, max_bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(Error::ImageLimit(format!(
            "{}: {} bytes exceeds the decode bound of {max_bytes} bytes",
            spec.id,
            bytes.len()
        )));
    }
    Ok(hex(&Sha256::digest(&bytes)))
}

/// The image's bytes: a path is read only when it is a regular file, and never past `max_bytes`
/// (one byte more, so the caller sees it is over), so a FIFO, a device or a huge file cannot hang
/// or exhaust the process; inline data longer than the bound could decode to is refused undecoded.
fn read_bytes(spec: &ImageSpec, base: &Path, max_bytes: u64) -> Result<Vec<u8>> {
    let bytes = match &spec.source {
        ImageSource::Path { path, sha256 } => {
            let full = if path.is_absolute() {
                path.clone()
            } else {
                base.join(path)
            };
            let cannot = |e: std::io::Error| {
                Error::Image(format!("{}: cannot read {}: {e}", spec.id, full.display()))
            };
            // Checked before opening: opening a FIFO blocks until a writer appears.
            let meta = std::fs::metadata(&full).map_err(cannot)?;
            if !meta.is_file() {
                return Err(Error::Image(format!(
                    "{}: {} is not a regular file",
                    spec.id,
                    full.display()
                )));
            }
            if meta.len() > max_bytes {
                return Err(Error::ImageLimit(format!(
                    "{}: {} bytes exceeds the decode bound of {max_bytes} bytes",
                    spec.id,
                    meta.len()
                )));
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&full)
                .and_then(|f| f.take(max_bytes + 1).read_to_end(&mut bytes))
                .map_err(cannot)?;
            if let Some(pinned) = sha256 {
                let actual = sha256_hex(&bytes);
                if !actual.eq_ignore_ascii_case(pinned) {
                    return Err(Error::Input(format!(
                        "{}: {} has sha256 {actual}, the job pins {pinned}",
                        spec.id,
                        full.display()
                    )));
                }
            }
            bytes
        }
        ImageSource::Base64 { base64, .. } => {
            let decoded_at_least = (base64.trim().len() as u64 / 4) * 3;
            if decoded_at_least.saturating_sub(2) > max_bytes {
                return Err(Error::ImageLimit(format!(
                    "{}: about {decoded_at_least} bytes exceeds the decode bound of {max_bytes} bytes",
                    spec.id
                )));
            }
            base64_decode(base64)
                .map_err(|e| Error::Image(format!("{}: invalid base64: {e}", spec.id)))?
        }
    };
    if let Some(expect) = &spec.expect {
        let actual = sha256_hex(&bytes);
        if actual != expect.file_sha256 {
            return Err(Error::Input(format!(
                "{}: the file has file_sha256 {actual}; the expected reference has {}",
                spec.id, expect.file_sha256
            )));
        }
    }
    Ok(bytes)
}

/// True when the bytes carry XMP GPano metadata declaring an equirectangular projection. A plain
/// byte scan, format-agnostic, which is enough for the two forms XMP uses (attribute and element).
fn has_gpano_equirectangular(bytes: &[u8]) -> bool {
    const KEY: &[u8] = b"GPano:ProjectionType";
    const VALUE: &[u8] = b"equirectangular";
    let mut from = 0;
    while let Some(pos) = find(&bytes[from..], KEY) {
        let start = from + pos + KEY.len();
        let end = (start + 64).min(bytes.len());
        if find(&bytes[start..end], VALUE).is_some() {
            return true;
        }
        from = start;
    }
    false
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Lowercase hex of a digest.
pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Standard base64 decoding; whitespace is ignored, padding is optional, anything else fails.
pub fn base64_decode(text: &str) -> std::result::Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for (i, c) in text.bytes().enumerate() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            other => return Err(format!("unexpected byte {other:#04x} at position {i}")),
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// Lowercase hex SHA-256 of bytes.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_every_remainder() {
        for len in 0..10usize {
            let bytes: Vec<u8> = (0..len as u8).map(|i| i.wrapping_mul(37)).collect();
            let text = base64_encode(&bytes);
            assert_eq!(
                base64_decode(&text).expect("decodes"),
                bytes,
                "len {len}: {text}"
            );
        }
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(
            base64_decode("TW Fu\n").expect("whitespace ignored"),
            b"Man"
        );
        assert!(base64_decode("TW*u").is_err());
    }

    #[test]
    fn gpano_scan_finds_attribute_and_element_forms() {
        assert!(has_gpano_equirectangular(
            br#"<rdf GPano:ProjectionType="equirectangular"/>"#
        ));
        assert!(has_gpano_equirectangular(
            b"<GPano:ProjectionType>equirectangular</GPano:ProjectionType>"
        ));
        assert!(!has_gpano_equirectangular(
            b"<GPano:ProjectionType>cylindrical</GPano:ProjectionType>"
        ));
        assert!(!has_gpano_equirectangular(b"no metadata here"));
    }

    #[test]
    fn hex_is_lowercase_and_zero_padded() {
        assert_eq!(hex(&[0, 1, 255, 16]), "0001ff10");
    }
}
