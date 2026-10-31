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

use std::path::Path;

use table_manners::images::{ImageTaken, Images, Locations};
use table_manners::{
    CONTRACT_VERSION, Cache, ErrorCode, ErrorInfo, Run, RunResult, Spend, SpendBasis, Status, Tool,
};

use crate::frames::LocationKind;
use crate::image::{CanonicalImage, hex};
use crate::job::{Projection, Role};
use crate::report::{Report, RootLocation};

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

/// The file every decision directory ends with: the contract's result (table-manners 2).
pub const RUN_FILE: &str = "run.json";

/// How a squint error class reads in the contract (table-manners 2.1, 2.2): the status, and for
/// `invalid` and `error` the code and whether an identical retry may succeed. A request that
/// could not be resolved (the job, the config, a credential) or an image that is not the one
/// named is the caller's to fix; a model or provider failure may pass on a retry.
pub fn status_of(class: &str) -> (Status, Option<(ErrorCode, bool)>) {
    match class {
        "cancelled" => (Status::Cancelled, None),
        "input" | "image_limit" => (Status::Invalid, Some((ErrorCode::Input, false))),
        "resolve" | "auth" => (Status::Invalid, Some((ErrorCode::Usage, false))),
        "provider" | "submission_invalid" | "no_submission" | "model_output" => {
            (Status::Error, Some((ErrorCode::Provider, true)))
        }
        "tool" => (Status::Error, Some((ErrorCode::ToolProcess, false))),
        "budget" => (Status::Error, Some((ErrorCode::Budget, false))),
        "io" => (Status::Error, Some((ErrorCode::Io, false))),
        _ => (Status::Error, Some((ErrorCode::Internal, false))),
    }
}

/// The exit code of an `invalid` request, whatever the error policy says: it is a bad request,
/// not a judgement (table-manners 6).
pub const EXIT_INVALID: i32 = 64;

/// The contract's result for a decision directory, from its report: the header the contract owns
/// and squint's body (the verdict, the exit code and the report's path). `started_ms` is when the
/// decision began; the result is finished now.
pub fn run_result(report: &Report, dir: &Path, started_ms: u64) -> RunResult {
    let finished_ms = now_ms();
    let id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let first_error = report.runs.iter().find_map(|r| {
        if r.terminal == crate::report::TERMINAL_CANCELLED {
            Some(("cancelled".to_string(), String::new()))
        } else {
            r.error
                .as_ref()
                .map(|e| (e.class.clone(), e.message.clone()))
        }
    });
    let (status, error) = match (&report.verdict, &first_error) {
        (Some(_), _) => (Status::Completed, None),
        (None, Some((class, message))) => {
            let (status, code) = status_of(class);
            let error = code.map(|(code, retryable)| ErrorInfo {
                code,
                retryable,
                reason: message.clone(),
            });
            (status, error)
        }
        (None, None) => (
            Status::Error,
            Some(ErrorInfo {
                code: ErrorCode::Internal,
                retryable: false,
                reason: "the decision ended without a verdict or an error".into(),
            }),
        ),
    };
    let images: &[CanonicalImage] = report.runs.first().map_or(&[], |r| &r.images);
    let found = report
        .runs
        .iter()
        .find(|r| r.verdict.is_some())
        .map(|run| {
            run.findings
                .iter()
                .filter_map(|finding| {
                    let root = finding.root.as_ref()?;
                    let image = images.iter().find(|i| i.id == root.image)?;
                    location(
                        image,
                        root,
                        &FindingWords {
                            label: &finding.reason,
                            disposition: &word(&finding.disposition),
                            confidence: &word(&finding.confidence),
                        },
                        &id,
                        &finding.obligation_ids.join(","),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut notes: Vec<String> = images.iter().flat_map(|i| i.warnings.clone()).collect();
    for run in &report.runs {
        for repair in &run.repairs {
            notes.push(format!(
                "run {}: repair after turn {} ({}, {})",
                run.run, repair.after_turn, repair.reason, repair.form
            ));
        }
    }
    let cache = match report.cache.status.as_str() {
        "hit" => Cache::Hit,
        "miss" => Cache::Miss,
        _ => Cache::Off,
    };
    let mut body = serde_json::Map::new();
    body.insert("verdict".into(), serde_json::json!(report.verdict));
    body.insert("exit_code".into(), report.exit_code.into());
    body.insert("agree".into(), serde_json::json!(report.decision.agree));
    body.insert(
        "report".into(),
        dir.join(crate::report::REPORT_FILE)
            .to_string_lossy()
            .into_owned()
            .into(),
    );
    RunResult {
        contract: CONTRACT_VERSION.into(),
        tool: Tool {
            name: "squint".into(),
            version: report.harness_version.clone(),
        },
        action: "check".into(),
        run: Run {
            resource: format!("squint://run/{id}/{RUN_FILE}"),
            id,
            dir: Some(dir.to_string_lossy().into_owned()),
        },
        status,
        error,
        started: rfc3339(started_ms),
        finished: rfc3339(finished_ms),
        duration_ms: finished_ms.saturating_sub(started_ms),
        input_identity: input_identity(report, images),
        spend: Spend {
            usd: report.cost.spent_usd,
            basis: SpendBasis::Measured,
        },
        cache,
        images: Some(Images {
            taken: images
                .iter()
                .map(|i| ImageTaken {
                    slot: role_word(i.role).into(),
                    role: role_word(i.role).into(),
                    image: image_reference(i),
                })
                .collect(),
            out: Vec::new(),
        }),
        locations: Some(Locations {
            used: Vec::new(),
            found,
        }),
        notes,
        body,
    }
}

/// Writes the result into the decision directory atomically: to a temporary name beside it, then
/// renamed, so a reader never sees half of one (table-manners 2).
pub fn write_run_result(dir: &Path, result: &RunResult) -> crate::Result<()> {
    let tmp = dir.join(format!(".{RUN_FILE}.tmp"));
    let mut text = serde_json::to_string_pretty(result)?;
    text.push('\n');
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, dir.join(RUN_FILE))?;
    Ok(())
}

fn role_word(role: Role) -> &'static str {
    match role {
        Role::Candidate => "candidate",
        Role::Source => "source",
        Role::Reference => "reference",
    }
}

/// A serde enum's word (`violated`, `confident`).
fn word<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// What this decision ran on: the images' digests, the job, the resolved rules, the redacted
/// config, the provider, the tools and the prompt and harness versions.
fn input_identity(report: &Report, images: &[CanonicalImage]) -> String {
    let identity = serde_json::json!({
        "images": images.iter().map(|i| (&i.id, &i.sha256)).collect::<Vec<_>>(),
        "job": report.job,
        "rules": report.rules,
        "config": report.config,
        "provider": report.provider,
        "tools": report.tools,
        "prompt_version": report.prompt_version,
        "harness_version": report.harness_version,
    });
    format!(
        "sha256:{}",
        hex(&Sha256::digest(identity.to_string().as_bytes()))
    )
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Milliseconds since the epoch as RFC 3339 in UTC (`2026-10-30T19:09:42.118Z`).
pub fn rfc3339(unix_ms: u64) -> String {
    let secs = unix_ms / 1000;
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Civil from days (Howard Hinnant's algorithm), proleptic Gregorian.
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        unix_ms % 1000
    )
}
