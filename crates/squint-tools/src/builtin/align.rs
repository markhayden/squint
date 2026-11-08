//! `align`: registers `image` onto `against` with the estimators of ADR 0014 and returns the
//! aligned frame `aligned:<image>:<transform>` (the image warped into `against`'s grid, lossless),
//! the transform with its quality, and the agreement measurements. A pair that does not register
//! is `not_applicable` with one `alignment_failure` region over the image; a textureless pair is
//! `not_applicable` with no region.

use image::RgbImage;
use squint_core::frames::Mapping;
use squint_core::frames::sphere;
use squint_core::job::Projection;
use squint_core::tool::envelope::{Measurement, Region, Transform};
use squint_core::tool::{Envelope, ImageHandle, Manifest, SquintContext, Status, Tool};
use squint_core::{Error, Result};

use super::raster;
use crate::align::{self, Options};

pub const MANIFEST: &str = include_str!("../../manifests/align.toml");

/// The aligned frame's longer side is capped here; its coordinates stay the parent's.
const MAX_SIDE: u32 = 1024;

pub struct Align {
    manifest: Manifest,
    options: Options,
}

impl Align {
    pub fn new() -> Align {
        Align::with_options(Options::default())
    }

    /// The tool under other estimator settings, for tests of its failure paths.
    pub fn with_options(options: Options) -> Align {
        Align {
            manifest: Manifest::from_toml(MANIFEST).expect("the align manifest is valid"),
            options,
        }
    }
}

impl Default for Align {
    fn default() -> Self {
        Align::new()
    }
}

fn load(id: &str, handle: &ImageHandle) -> Result<RgbImage> {
    let path = handle
        .path
        .as_deref()
        .ok_or_else(|| Error::Tool(format!("image {id:?} has no pixel path")))?;
    Ok(image::open(path)
        .map_err(|e| Error::Tool(format!("image {id:?}: cannot decode its pixels: {e}")))?
        .to_rgb8())
}

fn measurement(name: &str, value: f64, unit: &str) -> Measurement {
    Measurement {
        name: name.into(),
        value,
        unit: unit.into(),
        frame: None,
    }
}

impl Tool for Align {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn call(&self, args: &serde_json::Value, ctx: &SquintContext) -> Result<Envelope> {
        let started = std::time::Instant::now();
        let invalid = |m: String| Ok(super::invalid_args(&self.manifest, m));
        let id = |key: &str| args.get(key).and_then(|v| v.as_str());
        let (Some(image_id), Some(against_id)) = (id("image"), id("against")) else {
            return invalid("image and against must be image ids".into());
        };
        if image_id == against_id {
            return invalid("image and against must be two different images".into());
        }
        let (Some(image), Some(against)) = (ctx.images.get(image_id), ctx.images.get(against_id))
        else {
            return invalid(format!(
                "unknown image id: {}",
                [image_id, against_id]
                    .iter()
                    .filter(|i| !ctx.images.contains_key(**i))
                    .map(|i| format!("{i:?}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        };
        if image.projection != against.projection {
            return invalid(format!(
                "{image_id:?} is {:?} and {against_id:?} is {:?}; align needs one projection",
                image.projection, against.projection
            ));
        }
        let img = load(image_id, image)?;
        let tgt = load(against_id, against)?;
        let outcome = match image.projection {
            Projection::Flat => flat(&img, &tgt, &self.options)?,
            Projection::Equirectangular => spherical(&img, &tgt, &self.options)?,
        };
        let (status, quality, kind, params, mut measurements, mut summary) = match &outcome {
            Outcome::Registered {
                quality,
                kind,
                params,
                measurements,
                text,
                ..
            } => (
                align::Status::Registered,
                *quality,
                kind.clone(),
                params.clone(),
                measurements.clone(),
                text.clone(),
            ),
            Outcome::Failed {
                status,
                quality,
                text,
            } => (
                *status,
                *quality,
                String::new(),
                Vec::new(),
                Vec::new(),
                text.clone(),
            ),
        };
        measurements.extend([
            measurement("ncc", quality.ncc, "ratio"),
            measurement("inlier_fraction", quality.inlier_fraction, "ratio"),
            measurement("overlap", quality.overlap, "ratio"),
            measurement("texture", quality.texture, "intensity per pixel"),
        ]);
        let mut envelope = super::with_images(&self.manifest, String::new(), Vec::new(), started);
        envelope.measurements = measurements;
        match status {
            align::Status::Registered => {
                let Outcome::Registered { warped, .. } = &outcome else {
                    unreachable!("registered outcomes carry the warped image")
                };
                let transform_id = "t1".to_string();
                let frame = format!("aligned:{image_id}:{transform_id}");
                let mapping = match kind.as_str() {
                    "homography" => Mapping::Homography(params.clone()),
                    _ => Mapping::Rotation(params.clone()),
                };
                summary = format!(
                    "{image_id} registered onto {against_id}: {summary}. The aligned frame {frame} shares {against_id}'s grid; compare it with raw:{against_id}. Registration matches the camera, not the content: a structure reshaped inside the frame registers just as well, so a close fit is no evidence that structure held."
                );
                envelope.transforms.push(Transform {
                    id: transform_id,
                    from: format!("raw:{image_id}"),
                    to: format!("raw:{against_id}"),
                    kind,
                    params,
                    quality: serde_json::json!({
                        "ncc": quality.ncc,
                        "inlier_fraction": quality.inlier_fraction,
                        "overlap": quality.overlap,
                        "texture": quality.texture,
                    }),
                });
                let mut image_out = raster::frame(
                    &ctx.call_id,
                    0,
                    &format!("raw:{against_id}"),
                    mapping,
                    warped,
                )?;
                image_out.frame = frame;
                envelope.images.push(image_out);
            }
            align::Status::NotRegistered => {
                envelope.status = Status::NotApplicable;
                summary = format!(
                    "{image_id} does not register onto {against_id}: {summary}. The pair is not the same scene from the same viewpoint with its geometry intact."
                );
                envelope.regions.push(Region {
                    id: "r1".into(),
                    location: squint_core::frames::Location {
                        frame: format!("raw:{image_id}"),
                        kind: squint_core::frames::LocationKind::WholeImage,
                        bbox: None,
                        polygon: None,
                        yaw: None,
                        pitch: None,
                        fov: None,
                    },
                    label: format!("not registered onto {against_id}"),
                    score: (1.0 - quality.ncc.max(0.0)).clamp(0.0, 1.0),
                });
            }
            align::Status::LowTexture => {
                envelope.status = Status::NotApplicable;
                summary = format!(
                    "{image_id} and {against_id} could not be registered: {summary}. Nothing follows about their geometry."
                );
            }
            align::Status::NotConverged => {
                // A numerical failure, not evidence about the pair: no region.
                envelope.status = Status::NotApplicable;
                summary = format!(
                    "{image_id} and {against_id} could not be registered: registration did not converge ({summary}). Nothing follows about their geometry."
                );
            }
        }
        if envelope.measurements.iter().any(|m| !m.value.is_finite()) {
            return Err(Error::Tool(
                "align: a measurement is not a finite number".into(),
            ));
        }
        envelope.summary = summary;
        Ok(envelope)
    }
}

enum Outcome {
    Registered {
        quality: align::Quality,
        kind: String,
        params: Vec<f64>,
        measurements: Vec<Measurement>,
        text: String,
        warped: RgbImage,
    },
    Failed {
        status: align::Status,
        quality: align::Quality,
        text: String,
    },
}

/// The normalised homography (image `[0,1]²` to target `[0,1]²`) from a pixel one.
fn normalise_homography(h: &[f64; 9], img: &RgbImage, tgt: &RgbImage) -> Vec<f64> {
    let (iw, ih) = (f64::from(img.width()), f64::from(img.height()));
    let (tw, th) = (f64::from(tgt.width()), f64::from(tgt.height()));
    let to_pixels = [iw, 0.0, 0.0, 0.0, ih, 0.0, 0.0, 0.0, 1.0];
    let to_unit = [1.0 / tw, 0.0, 0.0, 0.0, 1.0 / th, 0.0, 0.0, 0.0, 1.0];
    let n = align::matmul(&to_unit, &align::matmul(h, &to_pixels));
    let n = if n[8].abs() > 1e-300 {
        n.map(|v| v / n[8])
    } else {
        n
    };
    n.to_vec()
}

fn flat(img: &RgbImage, tgt: &RgbImage, options: &Options) -> Result<Outcome> {
    let a = align::align_flat(img, tgt, options);
    let text_quality = |q: &align::Quality| {
        format!(
            "agreement {:.2}, inliers {:.2}, overlap {:.2}, texture {:.3}",
            q.ncc, q.inlier_fraction, q.overlap, q.texture
        )
    };
    Ok(match a.status {
        align::Status::Registered => {
            let h = a.homography;
            let (cx, cy) = (f64::from(img.width()) / 2.0, f64::from(img.height()) / 2.0);
            let m = Motion::of(&h, cx, cy);
            let side = tgt.width().max(tgt.height()).min(MAX_SIDE);
            let warped = warp_flat(img, tgt, &h, side)?;
            // A keystone is named as such: below a quarter of a percent it is resampling noise.
            let keystone = if m.perspective >= 0.0025 {
                format!(", perspective {:.1}%", m.perspective * 100.0)
            } else {
                String::new()
            };
            Outcome::Registered {
                quality: a.quality,
                kind: "homography".into(),
                params: normalise_homography(&h, img, tgt),
                measurements: vec![
                    measurement("translation_px", m.shift, "px"),
                    measurement("rotation_deg", m.rotation, "deg"),
                    measurement("scale", m.scale, "ratio"),
                    measurement("perspective", m.perspective, "ratio"),
                ],
                text: format!(
                    "shift {:.1} px, rotation {:.2}°, scale {:.3}{keystone}; {}",
                    m.shift,
                    m.rotation,
                    m.scale,
                    text_quality(&a.quality)
                ),
                warped,
            }
        }
        status => Outcome::Failed {
            status,
            quality: a.quality,
            text: match status {
                align::Status::LowTexture => {
                    format!("too little texture ({:.4})", a.quality.texture)
                }
                _ => text_quality(&a.quality),
            },
        },
    })
}

/// A pixel homography read as camera motion at the image centre `(cx, cy)`: the shift of the
/// centre, the rotation and scale of the local linear map there (the Jacobian, so a projective
/// term does not leak into them), and `perspective`, the largest relative change of the
/// projective depth from the centre to a corner (0 for an affine map; a keystone of k gives k).
struct Motion {
    shift: f64,
    rotation: f64,
    scale: f64,
    perspective: f64,
}

impl Motion {
    fn of(h: &[f64; 9], cx: f64, cy: f64) -> Motion {
        let depth = |x: f64, y: f64| h[6] * x + h[7] * y + h[8];
        let w = depth(cx, cy);
        let (px, py) = align::apply(h, cx, cy);
        // d(p)/d(x, y) = (A - p · gᵀ) / w, with A the top-left 2×2 and g = (h6, h7).
        let j = [
            (h[0] - px * h[6]) / w,
            (h[1] - px * h[7]) / w,
            (h[3] - py * h[6]) / w,
            (h[4] - py * h[7]) / w,
        ];
        let perspective = [
            (0.0, 0.0),
            (2.0 * cx, 0.0),
            (0.0, 2.0 * cy),
            (2.0 * cx, 2.0 * cy),
        ]
        .iter()
        .map(|&(x, y)| (depth(x, y) / w - 1.0).abs())
        .fold(0.0, f64::max);
        Motion {
            shift: (px - cx).hypot(py - cy),
            rotation: (j[2] - j[1]).atan2(j[0] + j[3]).to_degrees(),
            scale: (j[0] * j[3] - j[1] * j[2]).abs().sqrt(),
            perspective,
        }
    }
}

/// `img` resampled into `tgt`'s grid (at `side` on the longer edge) through the inverse of `h`
/// (image px → target px); pixels with no source are black.
fn warp_flat(img: &RgbImage, tgt: &RgbImage, h: &[f64; 9], side: u32) -> Result<RgbImage> {
    let scale = f64::from(side) / f64::from(tgt.width().max(tgt.height()));
    let (w, hh) = (
        ((f64::from(tgt.width()) * scale).round() as u32).max(1),
        ((f64::from(tgt.height()) * scale).round() as u32).max(1),
    );
    let inv = align::invert(h)
        .ok_or_else(|| Error::Tool("align: the registered homography is singular".into()))?;
    let mut out = RgbImage::new(w, hh);
    for (u, v, p) in out.enumerate_pixels_mut() {
        // Target pixel centre at full resolution, then back into the image.
        let (tx, ty) = ((f64::from(u) + 0.5) / scale, (f64::from(v) + 0.5) / scale);
        let (sx, sy) = align::apply(&inv, tx, ty);
        if sx < 0.0 || sy < 0.0 || sx > f64::from(img.width()) || sy > f64::from(img.height()) {
            continue;
        }
        *p = image::Rgb(raster::sample(img, sx, sy, false));
    }
    Ok(out)
}

fn spherical(img: &RgbImage, tgt: &RgbImage, options: &Options) -> Result<Outcome> {
    let a = align::align_sphere(img, tgt, options);
    let text_quality = |q: &align::Quality| {
        format!(
            "agreement {:.2}, inliers {:.2}, texture {:.3}",
            q.ncc, q.inlier_fraction, q.texture
        )
    };
    Ok(match a.status {
        align::Status::Registered => {
            let side = tgt.width().min(MAX_SIDE);
            let warped = warp_sphere(img, tgt, &a.rotation, side);
            Outcome::Registered {
                quality: a.quality,
                kind: "rotation".into(),
                params: a.rotation.to_vec(),
                measurements: vec![
                    measurement("yaw_deg", a.yaw_deg, "deg"),
                    measurement("pitch_deg", a.pitch_deg, "deg"),
                    measurement("roll_deg", a.roll_deg, "deg"),
                ],
                text: format!(
                    "yaw {:.2}°, pitch {:.2}°, roll {:.2}°; {}",
                    a.yaw_deg,
                    a.pitch_deg,
                    a.roll_deg,
                    text_quality(&a.quality)
                ),
                warped,
            }
        }
        status => Outcome::Failed {
            status,
            quality: a.quality,
            text: match status {
                align::Status::LowTexture => {
                    format!("too little texture ({:.4})", a.quality.texture)
                }
                _ => text_quality(&a.quality),
            },
        },
    })
}

/// `img` resampled into `tgt`'s equirectangular grid: a target direction `q` shows the image at
/// `rᵀ q`, since `r` takes image directions to target directions.
fn warp_sphere(img: &RgbImage, tgt: &RgbImage, r: &[f64; 9], side: u32) -> RgbImage {
    let w = side.max(2);
    let h = (w / 2).max(1);
    let _ = tgt;
    let rt = [r[0], r[3], r[6], r[1], r[4], r[7], r[2], r[5], r[8]];
    let (iw, ih) = (f64::from(img.width()), f64::from(img.height()));
    let mut out = RgbImage::new(w, h);
    for (u, v, p) in out.enumerate_pixels_mut() {
        let q = sphere::direction_of_equirect(
            (f64::from(u) + 0.5) / f64::from(w),
            (f64::from(v) + 0.5) / f64::from(h),
        );
        let d = [
            rt[0] * q[0] + rt[1] * q[1] + rt[2] * q[2],
            rt[3] * q[0] + rt[4] * q[1] + rt[5] * q[2],
            rt[6] * q[0] + rt[7] * q[1] + rt[8] * q[2],
        ];
        let e = sphere::equirect_of(d);
        *p = image::Rgb(raster::sample(img, e[0] * iw, e[1] * ih, true));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::Motion;

    /// `m` conjugated about the centre `(cx, cy)`: T(c) · m · T(-c).
    fn about(m: [f64; 9], cx: f64, cy: f64) -> [f64; 9] {
        let t = [1.0, 0.0, cx, 0.0, 1.0, cy, 0.0, 0.0, 1.0];
        let u = [1.0, 0.0, -cx, 0.0, 1.0, -cy, 0.0, 0.0, 1.0];
        crate::align::matmul(&t, &crate::align::matmul(&m, &u))
    }

    #[test]
    fn a_keystone_reads_as_perspective_not_rotation_or_scale() {
        let (w, h) = (512.0, 384.0);
        let k = 0.06;
        let m = about(
            [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 2.0 * k / w, 0.0, 1.0],
            w / 2.0,
            h / 2.0,
        );
        let r = Motion::of(&m, w / 2.0, h / 2.0);
        assert!(
            r.shift < 1e-9 && r.rotation.abs() < 1e-9,
            "{} {}",
            r.shift,
            r.rotation
        );
        assert!((r.scale - 1.0).abs() < 1e-9 && (r.perspective - k).abs() < 1e-9);
    }

    #[test]
    fn a_similarity_reads_as_its_rotation_and_scale_with_no_perspective() {
        let (w, h) = (512.0, 384.0);
        let (deg, s) = (5.0f64, 0.9);
        let (c, n) = (s * deg.to_radians().cos(), s * deg.to_radians().sin());
        let m = about([c, -n, 8.0, n, c, 4.0, 0.0, 0.0, 1.0], w / 2.0, h / 2.0);
        let r = Motion::of(&m, w / 2.0, h / 2.0);
        assert!((r.rotation - deg).abs() < 1e-9 && (r.scale - s).abs() < 1e-9);
        assert!((r.shift - 80f64.sqrt()).abs() < 1e-9 && r.perspective < 1e-12);
    }
}
