//! squint's side of the table-manners contract (ADR 0018, SPEC "External interface"): the
//! contract's objects built from squint's own. The types and schemas are the `table-manners`
//! crate's; this module only fills them in.

use std::path::Path;

use image::DynamicImage;
use image::metadata::Orientation;
use sha2::{Digest, Sha256};
use table_manners::images::{
    BboxPart, ImageReference, ImageSlot, ImageTaken, Images, Limits, Location,
    LocationKind as ContractKind, LocationOutput, Locations, Outputs, Part, PolygonPart,
    Projection as ContractProjection, Source, Topology, Vocabulary,
};
use table_manners::{
    Action, CONTRACT_VERSION, Cache, Cost, DescribedTool, Description, Duration, ErrorCode,
    ErrorInfo, Profile, Readiness, Render, RenderKind, Run, RunResult, Spend, SpendBasis, Status,
    Tool,
};

use crate::aggregate::Confidence;
use crate::config::Config;
use crate::frames::LocationKind;
use crate::image::{CanonicalImage, hex};
use crate::job::{Projection, Role};
use crate::obligations::{MergeInput, RulesFile, SHIPPED_BASES, shipped};
use crate::report::{Report, RootLocation};
use crate::submission::FindingDisposition;

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
    // Only a panorama wraps; a bounded image has no seam to split at (table-manners 9.2).
    let seam_parts = root
        .seam_parts
        .as_ref()
        .filter(|_| image.projection == Projection::Equirectangular);
    let (kind, parts, straddles_wrap) = match (root.kind, seam_parts) {
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

/// An amount as the contract carries it: US dollars to at most six decimals (table-manners 4.6).
pub fn usd(amount: f64) -> f64 {
    (amount * 1e6).round() / 1e6
}

/// The file every decision directory ends with: the contract's result (table-manners 2).
pub const RUN_FILE: &str = "run.json";

/// The file a served run's association is written to when its directory is made, before any
/// paid work (table-manners 4.3): a run still going is listed with it.
pub const ASSOCIATION_FILE: &str = "association.json";

/// Which serving session and client request a run belongs to (table-manners 4.3, 4.4). Both are
/// null for a run made outside a server (the CLI).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Association {
    pub session_id: Option<String>,
    pub client_run_id: Option<String>,
    /// The action the run is for, as `describe` names it, so a run still going is listed under it.
    #[serde(default)]
    pub action: Option<String>,
}

impl Association {
    /// Written into a run's directory as soon as it exists, whole: to a temporary name, then
    /// renamed, so a reader never sees half of it.
    pub fn write(&self, dir: &Path) -> crate::Result<()> {
        let tmp = dir.join(format!(".{ASSOCIATION_FILE}.tmp"));
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, dir.join(ASSOCIATION_FILE))?;
        Ok(())
    }

    /// A run's association, or none (both null) for a run made outside a server.
    pub fn read(dir: &Path) -> Association {
        std::fs::read(dir.join(ASSOCIATION_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}

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
        "cap" => (Status::Error, Some((ErrorCode::Cap, false))),
        "stale_estimate" => (Status::Invalid, Some((ErrorCode::StaleEstimate, false))),
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
    let association = Association::read(dir);
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
        // The use case's action, as `describe` names it, whether the check came over MCP or not.
        action: report.use_case.replace('-', "_"),
        run: Run {
            resource: format!("squint://run/{id}/{RUN_FILE}"),
            id,
            dir: Some(dir.to_string_lossy().into_owned()),
            session_id: association.session_id,
            client_run_id: association.client_run_id,
        },
        status,
        error,
        started: rfc3339(started_ms),
        finished: rfc3339(finished_ms),
        duration_ms: finished_ms.saturating_sub(started_ms),
        input_identity: input_identity(report, images),
        spend: Spend {
            // A cache hit spends nothing; the cached report's spend was its first run's.
            usd: if cache == Cache::Hit {
                0.0
            } else {
                usd(report.cost.spent_usd)
            },
            unresolved_usd: if cache == Cache::Hit {
                0.0
            } else {
                usd(report.cost.unresolved_usd)
            },
            basis: if report.cost.estimated && cache != Cache::Hit {
                SpendBasis::Estimated
            } else {
                SpendBasis::Measured
            },
        },
        cache,
        images: Some(Images {
            taken: images
                .iter()
                .map(|i| ImageTaken {
                    slot: slot_word(i.role).into(),
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

/// The slot an image of `role` comes in through, as `describe` declares it: references share the
/// one `many` slot, which `images.in` lists once per image, in the order given (table-manners
/// 11.1).
fn slot_word(role: Role) -> &'static str {
    match role {
        Role::Reference => "references",
        other => role_word(other),
    }
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
/// config, the provider, the tools and the prompt and harness versions. How an image arrived (a
/// path or inline bytes) is left out: its digest already says what it was.
fn input_identity(report: &Report, images: &[CanonicalImage]) -> String {
    let mut job = serde_json::to_value(&report.job).unwrap_or_default();
    if let Some(list) = job["images"].as_array_mut() {
        for image in list {
            if let Some(fields) = image.as_object_mut() {
                fields.remove("source");
            }
        }
    }
    let identity = serde_json::json!({
        "images": images.iter().map(|i| (&i.id, &i.sha256)).collect::<Vec<_>>(),
        "job": job,
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

/// Milliseconds since the epoch.
pub fn now_ms() -> u64 {
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

/// The longest gap between progress notifications on a live run (table-manners 4.1).
pub const PROGRESS_INTERVAL_S: u32 = 10;

/// What a check runs on, as far as the description's cost must say (table-manners 3): a
/// provider that may charge (any catalog profile, or one not chosen yet with no configuration),
/// or one that never can (the scripted fake). Readiness is [`crate::readiness`]'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Executor {
    Paid,
    Free,
}

/// squint's description document (table-manners 3 and 11.2): one action per use case a check can
/// run (the one `[rules]` names, else every shipped base, which is also what squint describes
/// with no configuration), taking the images its rules require (`estimate` is reserved and never
/// an action); every disposition and confidence word a finding can carry; the inline limit from
/// the decode bound (the default one with no configuration). `executor` says what a check will
/// run on, so the declared cost is the truth of this configuration (squint issue #2); every
/// action carries `readiness` ([`crate::readiness`]).
pub fn description(
    config: Option<&Config>,
    version: &str,
    executor: Executor,
    readiness: &Readiness,
) -> crate::Result<Description> {
    let actions: Vec<Action> = use_cases(config)?
        .iter()
        .map(|(base, requires_roles)| {
            let base = base.as_str();
            let required = |role: Role| requires_roles.contains(&role);
            let mut images = vec![slot("candidate", "candidate", true)];
            if required(Role::Source) {
                images.push(slot("source", "source", true));
            }
            images.push(references_slot(required(Role::Reference)));
            Action {
                name: base.replace('-', "_"),
                title: title(base),
                purpose: purpose(base).into(),
                params_schema: params_schema(),
                images: Some(images),
                locations: Some(Vec::new()),
                outputs: Some(Outputs {
                    images: Vec::new(),
                    locations: vec![LocationOutput::Found],
                }),
                cost: match executor {
                    Executor::Paid => Cost {
                        may_spend: true,
                        basis: Some(SpendBasis::Measured),
                    },
                    Executor::Free => Cost {
                        may_spend: false,
                        basis: None,
                    },
                },
                readiness: readiness.clone(),
                duration: Duration {
                    typical_s: 60.0,
                    max_s: 900.0,
                },
                // Each render runs the tool's own executable, `command[0]` being `tool.name`, with
                // the contract's placeholders only as whole arguments (table-manners 3.2).
                renders: vec![
                    Render {
                        name: "report".into(),
                        kind: RenderKind::Server,
                        ready_timeout_s: Some(10),
                        command: [
                            "squint",
                            "report",
                            "view",
                            "{run.dir}",
                            "--port",
                            "0",
                            "--frame-ancestor",
                            "{frame_ancestor}",
                        ]
                        .map(String::from)
                        .to_vec(),
                    },
                    Render {
                        name: "export".into(),
                        kind: RenderKind::Document,
                        ready_timeout_s: None,
                        command: ["squint", "report", "export", "{run.dir}"]
                            .map(String::from)
                            .to_vec(),
                    },
                ],
            }
        })
        .collect();
    let mut extensions = std::collections::BTreeMap::new();
    extensions.insert(
        "x-squint".into(),
        serde_json::json!({
            "self_description": [
                "squint rules show --json",
                "squint tools list --json",
                "squint catalog show"
            ]
        }),
    );
    Ok(Description {
        contract: CONTRACT_VERSION.into(),
        profiles: vec![Profile::Images],
        tool: DescribedTool {
            name: "squint".into(),
            version: version.into(),
            purpose: "Checks an image against what was asked of it.".into(),
        },
        actions,
        vocabulary: Some(Vocabulary {
            disposition: [
                FindingDisposition::Violation,
                FindingDisposition::AllowedChange,
                FindingDisposition::CantTell,
                FindingDisposition::Observed,
            ]
            .iter()
            .map(word)
            .collect(),
            confidence: [Confidence::Confident, Confidence::Tentative]
                .iter()
                .map(word)
                .collect(),
            evidence: Vec::new(),
            score_kind: Vec::new(),
        }),
        limits: Some(Limits {
            inline_bytes: config.map_or_else(
                || crate::config::ImagesConfig::default().max_decode_bytes,
                |c| c.images.max_decode_bytes,
            ),
        }),
        progress_interval_s: PROGRESS_INTERVAL_S,
        resources: [
            "squint://runs",
            "squint://session",
            "squint://run/{id}/run.json",
            "squint://run/{id}/report.json",
            "squint://run/{id}/journal",
        ]
        .map(String::from)
        .to_vec(),
        extensions,
    })
}

/// The use cases a check can run, each with the roles its rules require: the one `[rules]`
/// names, merged with its profile and extra files (resolved as `check` resolves them, against the
/// working directory), else (and with no configuration) every shipped base.
fn use_cases(config: Option<&Config>) -> crate::Result<Vec<(String, Vec<Role>)>> {
    let Some(rules) = config.and_then(|c| c.rules.as_ref()) else {
        return Ok(SHIPPED_BASES
            .iter()
            .filter_map(|base| {
                let file = RulesFile::parse(shipped(base)?, base).ok()?;
                Some((base.to_string(), file.rules.requires_roles))
            })
            .collect());
    };
    let extra = rules
        .extra
        .iter()
        .map(|path| RulesFile::load(path))
        .collect::<crate::Result<Vec<_>>>()?;
    let names: Vec<String> = rules
        .extra
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let merged = crate::obligations::merge(&MergeInput {
        base: &rules.base,
        profile: rules.profile.as_deref(),
        extra: &extra,
        extra_names: &names,
        job: None,
        enabled_tools: &[],
    })?;
    Ok(vec![(rules.base.clone(), merged.requires_roles)])
}

/// What each shipped use case checks, in one sentence. Kept here rather than in the rule sets:
/// a sealed evaluation set binds the rule files byte for byte.
fn purpose(base: &str) -> &'static str {
    match base {
        "source-fidelity" => "An edited or re-generated image kept what mattered in the original.",
        "reference-consistency" => {
            "A subject from one or more reference images appears faithfully in a new scene."
        }
        "reality-checks" => "The image makes no mistake a real photograph would not make.",
        "brief-compliance" => "The image contains what was asked for and nothing it should not.",
        "product-fidelity" => "A product shot keeps the exact product shown in the references.",
        _ => "Checks an image against its use case's rule set.",
    }
}

/// A slot that takes one image in.
fn slot(slot: &str, role: &str, required: bool) -> ImageSlot {
    ImageSlot {
        slot: slot.into(),
        role: role.into(),
        required,
        many: false,
    }
}

/// `references`: a `many` slot, a non-empty list of images in, one per reference (table-manners
/// 10.1, 11.2).
fn references_slot(required: bool) -> ImageSlot {
    ImageSlot {
        many: true,
        ..slot("references", "reference", required)
    }
}

/// The `arguments` the reserved `estimate` tool takes (table-manners 4.5): exactly what a call of
/// any action would send, so its parameters and image slots, `references` a `many` slot, every
/// slot optional but the candidate (whether an action needs its source is that action's to say,
/// when the arguments are checked as its call's).
pub fn estimate_arguments() -> (serde_json::Value, Vec<ImageSlot>) {
    (
        params_schema(),
        vec![
            slot("candidate", "candidate", true),
            slot("source", "source", false),
            references_slot(false),
        ],
    )
}

/// `source-fidelity` reads as "Source fidelity".
fn title(base: &str) -> String {
    let words = base.replace('-', " ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

/// An obligation's JSON Schema, self-contained (its definitions inlined under `$defs`), so a
/// client can build a requirement editor without reading squint's rule files.
/// One job requirement's schema and the definitions it refers to. The definitions are returned
/// apart: its references (`#/$defs/Kind`) resolve from the root of whatever schema embeds it, so
/// the embedding schema carries them at its root (squint issue #2).
fn obligation_schema() -> (serde_json::Value, serde_json::Value) {
    let mut schema = serde_json::to_value(schemars::schema_for!(crate::obligations::Obligation))
        .unwrap_or_default();
    let mut defs = serde_json::Value::Object(Default::default());
    if let Some(map) = schema.as_object_mut() {
        map.remove("$schema");
        if let Some(d) = map.remove("$defs") {
            defs = d;
        }
    }
    (schema, defs)
}

/// The arguments of a check beside its images: the brief, the job's requirements, a lower spend
/// cap.
fn params_schema() -> serde_json::Value {
    let (obligation, defs) = obligation_schema();
    let properties = serde_json::json!({
        "brief": { "type": "string", "description": "What was asked for, in words." },
        "requirements": {
            "type": "array",
            "items": obligation,
            "description": "Job requirements, as `[[job.requirements]]` in a job file; ids `RQ<n>`."
        },
        "max_spend_usd": {
            "type": "number",
            "exclusiveMinimum": 0,
            "description": "Lowers the configured per-call cap; a value above it is refused."
        }
    });
    serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": [],
        "additionalProperties": false,
        "$defs": defs
    })
}

/// The arguments a call may carry beside its images (`params_schema`, and the locator and
/// image-block choices every images action takes, table-manners 10.2).
const CALL_ARGUMENTS: &[&str] = &[
    "candidate",
    "source",
    "references",
    "brief",
    "requirements",
    "max_spend_usd",
    "return_images",
    "image_blocks",
];

/// A job from a contract call's arguments (table-manners 10.1): `candidate` and an optional
/// `source`, each one image in, and `references`, a non-empty list of them (`ref:1`, `ref:2`, …,
/// in the order given); the brief and requirements as in a job file. A path must be absolute;
/// inline bytes become a base64 image; an `expect` is kept, and its projection is the image's.
/// Anything else, an empty `references`, or a malformed image in, is a usage error.
pub fn job_from_call(
    use_case: &str,
    args: &serde_json::Map<String, serde_json::Value>,
) -> crate::Result<crate::job::Job> {
    use crate::Error;
    use crate::job::{ImageSpec, Job};

    if let Some(unknown) = args.keys().find(|k| !CALL_ARGUMENTS.contains(&k.as_str())) {
        return Err(Error::Job(format!("unknown argument {unknown:?}")));
    }
    let image = |id: String, role: Role, value: &serde_json::Value| -> crate::Result<ImageSpec> {
        let image_in: table_manners::images::ImageIn = serde_json::from_value(value.clone())
            .map_err(|e| {
                Error::Job(format!(
                    "{id}: an image is {{ \"path\" }} or {{ \"inline\" }} with an optional \"expect\": {e}"
                ))
            })?;
        let (source, expect) = match image_in {
            table_manners::images::ImageIn::Path(p) => {
                if !Path::new(&p.path).is_absolute() {
                    return Err(Error::Input(format!(
                        "{id}: the path {:?} is not absolute",
                        p.path
                    )));
                }
                (
                    crate::job::ImageSource::Path {
                        path: p.path.into(),
                        sha256: None,
                    },
                    p.expect,
                )
            }
            table_manners::images::ImageIn::Inline(i) => {
                crate::image::base64_decode(&i.inline.data)
                    .map_err(|e| Error::Input(format!("{id}: inline data: {e}")))?;
                (
                    crate::job::ImageSource::Base64 {
                        base64: i.inline.data,
                        media_type: i.inline.media_type,
                    },
                    i.expect,
                )
            }
        };
        let projection = expect.as_ref().map(|e| match e.projection {
            ContractProjection::Flat => Projection::Flat,
            ContractProjection::Equirectangular => Projection::Equirectangular,
        });
        Ok(ImageSpec {
            label: (role == Role::Reference).then(|| id.trim_start_matches("ref:").to_string()),
            id,
            role,
            source,
            projection,
            expect,
        })
    };
    let candidate = args
        .get("candidate")
        .ok_or_else(|| Error::Job("candidate: required".into()))?;
    let mut images = vec![image("candidate".into(), Role::Candidate, candidate)?];
    if let Some(source) = args.get("source") {
        images.push(image("source".into(), Role::Source, source)?);
    }
    if let Some(references) = args.get("references") {
        let list = references
            .as_array()
            .filter(|list| !list.is_empty())
            .ok_or_else(|| Error::Job("references: a non-empty list of images".into()))?;
        for (n, value) in list.iter().enumerate() {
            images.push(image(format!("ref:{}", n + 1), Role::Reference, value)?);
        }
    }
    let brief = match args.get("brief") {
        None => None,
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(Error::Job("brief: a string".into())),
    };
    let requirements = match args.get("requirements") {
        None => Vec::new(),
        Some(v) => serde_json::from_value(v.clone())
            .map_err(|e| Error::Job(format!("requirements: {e}")))?,
    };
    let job = Job {
        use_case: use_case.to_string(),
        brief,
        requirements,
        images,
    };
    job.validate()?;
    Ok(job)
}

/// How long the server honours an estimate's binding: one hour (table-manners 4.5, ADR 0018
/// amendment), after which prices or the catalog may have moved.
pub const ESTIMATE_TTL_MS: u64 = 3_600_000;

/// A JSON value as canonical text: object keys sorted at every level, no whitespace, so two equal
/// documents digest equal however their keys arrived.
fn canonical(value: &serde_json::Value) -> String {
    fn sorted(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                let mut out = serde_json::Map::new();
                for key in keys {
                    out.insert(key.clone(), sorted(&map[key]));
                }
                serde_json::Value::Object(out)
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(sorted).collect())
            }
            other => other.clone(),
        }
    }
    // Keys are written in insertion order whether or not serde_json keeps it, so sorting once
    // and writing by hand gives the same text under either feature.
    fn write(value: &serde_json::Value, out: &mut String) {
        match value {
            serde_json::Value::Object(map) => {
                out.push('{');
                for (n, (key, item)) in map.iter().enumerate() {
                    if n > 0 {
                        out.push(',');
                    }
                    out.push_str(&serde_json::Value::String(key.clone()).to_string());
                    out.push(':');
                    write(item, out);
                }
                out.push('}');
            }
            serde_json::Value::Array(items) => {
                out.push('[');
                for (n, item) in items.iter().enumerate() {
                    if n > 0 {
                        out.push(',');
                    }
                    write(item, out);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut out = String::new();
    write(&sorted(value), &mut out);
    out
}

/// `sha256:<hex>` of a value's canonical text.
fn digest(value: &serde_json::Value) -> String {
    format!(
        "sha256:{}",
        hex(&Sha256::digest(canonical(value).as_bytes()))
    )
}

/// The identity of the configuration in force (table-manners 4.5 `effective.config_identity`):
/// the digest of the redacted configuration's canonical JSON, so a secret never shapes it and the
/// same settings read from anywhere are the same identity.
pub fn config_identity(config: &Config) -> String {
    digest(&config.redacted_json())
}

/// What an estimate's binding covers beside the call (table-manners 4.5): the tool version, the
/// configuration's identity (none without one) and the session (none from the command line).
pub struct Binder<'a> {
    pub version: &'a str,
    pub config_identity: Option<&'a str>,
    pub session_id: Option<&'a str>,
}

impl Binder<'_> {
    /// The binding of one call of `action` (table-manners 4.5, ADR 0018 amendment): sha256 over
    /// the canonical JSON of the action, the call's arguments with every image replaced by its
    /// file digest (`file_digests`, one per job image in order, so a path and inline bytes of the
    /// same file bind equal) and its `expect` kept, the tool version, the configuration's identity
    /// and the session id. `return_images` and `image_blocks` only choose how results are
    /// delivered and are not part of it; an empty `requirements` is the same call as none.
    pub fn binding(
        &self,
        action: &str,
        job: &crate::job::Job,
        file_digests: &[String],
        max_spend_usd: Option<f64>,
    ) -> String {
        let image = |n: usize| {
            let spec = &job.images[n];
            let mut bound = serde_json::Map::new();
            bound.insert(
                "file_sha256".into(),
                file_digests.get(n).cloned().unwrap_or_default().into(),
            );
            if let Some(expect) = &spec.expect {
                bound.insert(
                    "expect".into(),
                    serde_json::to_value(expect).unwrap_or_default(),
                );
            }
            serde_json::Value::Object(bound)
        };
        let of_role = |role: Role| -> Vec<serde_json::Value> {
            (0..job.images.len())
                .filter(|&n| job.images[n].role == role)
                .map(image)
                .collect()
        };
        let mut arguments = serde_json::Map::new();
        if let Some(candidate) = of_role(Role::Candidate).pop() {
            arguments.insert("candidate".into(), candidate);
        }
        if let Some(source) = of_role(Role::Source).pop() {
            arguments.insert("source".into(), source);
        }
        let references = of_role(Role::Reference);
        if !references.is_empty() {
            arguments.insert("references".into(), references.into());
        }
        if let Some(brief) = &job.brief {
            arguments.insert("brief".into(), brief.clone().into());
        }
        if !job.requirements.is_empty() {
            arguments.insert(
                "requirements".into(),
                serde_json::to_value(&job.requirements).unwrap_or_default(),
            );
        }
        if let Some(max) = max_spend_usd {
            arguments.insert("max_spend_usd".into(), max.into());
        }
        digest(&serde_json::json!({
            "action": action,
            "arguments": arguments,
            "tool_version": self.version,
            "config_identity": self.config_identity,
            "session_id": self.session_id,
        }))
    }
}

/// What an estimate is computed under (table-manners 4.5): the configuration in force (none when
/// none loaded), its tools, the action's readiness, the prices the limits check would plan with
/// (none when the call cannot be priced), the session observed once (none from the command line)
/// and the clock.
pub struct EstimateContext<'a> {
    pub binder: Binder<'a>,
    pub config: Option<&'a Config>,
    pub tools: &'a crate::tool::ToolRegistry,
    pub readiness: Readiness,
    pub pricing: Option<&'a dyn crate::ledger::Pricing>,
    pub session: Option<table_manners::Session>,
    pub now_ms: u64,
    /// How long the binding is honoured; [`ESTIMATE_TTL_MS`] outside tests.
    pub ttl_ms: u64,
}

/// The estimate of one call of `action` (table-manners 4.5): never a run, never a provider call.
/// The images are read and checked as a check reads them (an `expect` that does not match is
/// `input`), the rules resolved when a configuration is loaded, and the worst case planned by the
/// same envelope the limits check reserves. A call that cannot be priced (no configuration, or no
/// prices for its model) is refused for readiness with nulls, never a made-up zero; otherwise the
/// refusal is the first of readiness, cap and budget that applies. `max_spend_usd` above the
/// configured cap is a usage error, never silently lowered.
pub fn estimate(
    ctx: &EstimateContext<'_>,
    action: &str,
    job: &crate::job::Job,
    job_base: &Path,
    max_spend_usd: Option<f64>,
) -> crate::Result<table_manners::Estimate> {
    use table_manners::{Effective, Estimate, EstimateRefusal, ReadinessState};
    // With no configuration, squint's defaults stand for the cap and the image bounds.
    let configured_cap = ctx.config.map_or_else(
        || crate::config::BudgetConfig::default().spend_cap_usd,
        |c| c.budget.spend_cap_usd,
    );
    let cap = crate::config::call_cap(configured_cap, max_spend_usd)?;
    let (images, setup) = match ctx.config {
        Some(config) => {
            let setup = crate::run::setup(config, job, job_base, ctx.tools)?;
            (setup.images.clone(), Some(setup))
        }
        None => {
            let bounds = crate::config::ImagesConfig::default().decode_bounds();
            let images = job
                .images
                .iter()
                .map(|spec| crate::image::load(spec, job_base, &bounds))
                .collect::<crate::Result<Vec<_>>>()?;
            (images, None)
        }
    };
    let file_digests: Vec<String> = images.iter().map(|i| i.sha256.clone()).collect();
    let envelope = match (ctx.config, &setup, ctx.pricing) {
        (Some(config), Some(setup), Some(pricing)) => Some(crate::run::envelope_of(
            config, setup, job, ctx.tools, pricing,
        )?),
        _ => None,
    };
    let worst = envelope.as_ref().map(|e| usd(e.usd));
    let remaining = ctx.session.as_ref().and_then(|s| s.remaining_usd);
    // Rounding slack, as the ledgers compare.
    const EPSILON: f64 = 1e-9;
    let refused = match worst {
        None => Some(EstimateRefusal::Readiness),
        Some(_) if ctx.readiness.state != ReadinessState::Ready => Some(EstimateRefusal::Readiness),
        Some(w) if w > cap + EPSILON => Some(EstimateRefusal::Cap),
        Some(w) if remaining.is_some_and(|r| w > r + EPSILON) => Some(EstimateRefusal::Budget),
        Some(_) => None,
    };
    let mut extensions = std::collections::BTreeMap::new();
    if let Some(e) = &envelope {
        extensions.insert(
            "x-squint".into(),
            serde_json::json!({
                "decision_runs": e.decision_runs,
                "requests": e.requests.len(),
                "attempts": e.attempts,
            }),
        );
    }
    Ok(Estimate {
        contract: CONTRACT_VERSION.into(),
        tool: Tool {
            name: "squint".into(),
            version: ctx.binder.version.into(),
        },
        action: action.into(),
        binding: ctx
            .binder
            .binding(action, job, &file_digests, max_spend_usd),
        session_id: ctx.binder.session_id.map(str::to_string),
        effective: Effective {
            model: envelope
                .as_ref()
                .and(ctx.config)
                .map(|c| c.provider.model.clone()),
            config_identity: ctx.binder.config_identity.map(str::to_string),
            extensions,
        },
        worst_case_usd: worst,
        cap_usd: Some(usd(cap)),
        allowed: refused.is_none(),
        refused,
        readiness: ctx.readiness.clone(),
        session: ctx.session.clone(),
        expires: rfc3339(ctx.now_ms.saturating_add(ctx.ttl_ms)),
    })
}

/// The result of a check refused before its decision was allocated (its arguments, a path outside
/// the roots, its provider): the header with no spend and no images or locations; `error` sets
/// the status from its class. The input identity is a digest of the call's arguments.
#[allow(clippy::too_many_arguments)]
pub fn action_result(
    dir: &Path,
    version: &str,
    action: &str,
    association: &Association,
    started_ms: u64,
    input: &serde_json::Value,
    error: Option<&crate::Error>,
    body: serde_json::Map<String, serde_json::Value>,
) -> RunResult {
    let finished_ms = now_ms();
    let id = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (status, error) = match error {
        None => (Status::Completed, None),
        Some(e) => {
            let (status, code) = status_of(e.class());
            (
                status,
                code.map(|(code, retryable)| ErrorInfo {
                    code,
                    retryable,
                    reason: e.to_string(),
                }),
            )
        }
    };
    RunResult {
        contract: CONTRACT_VERSION.into(),
        tool: Tool {
            name: "squint".into(),
            version: version.into(),
        },
        action: action.into(),
        run: Run {
            resource: format!("squint://run/{id}/{RUN_FILE}"),
            id,
            dir: Some(dir.to_string_lossy().into_owned()),
            session_id: association.session_id.clone(),
            client_run_id: association.client_run_id.clone(),
        },
        status,
        error,
        started: rfc3339(started_ms),
        finished: rfc3339(finished_ms),
        duration_ms: finished_ms.saturating_sub(started_ms),
        input_identity: format!(
            "sha256:{}",
            hex(&Sha256::digest(input.to_string().as_bytes()))
        ),
        spend: Spend {
            usd: 0.0,
            unresolved_usd: 0.0,
            basis: SpendBasis::Measured,
        },
        cache: Cache::Off,
        images: Some(Images {
            taken: Vec::new(),
            out: Vec::new(),
        }),
        locations: Some(Locations {
            used: Vec::new(),
            found: Vec::new(),
        }),
        notes: Vec::new(),
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    /// The contract status and code each error must become (table-manners 2.2). The match is
    /// exhaustive, so a new error variant does not compile until its mapping is decided here.
    fn expected(e: &Error) -> (Status, Option<(ErrorCode, bool)>) {
        match e {
            Error::Cancelled(_) => (Status::Cancelled, None),
            Error::Input(_) | Error::ImageLimit(_) => {
                (Status::Invalid, Some((ErrorCode::Input, false)))
            }
            Error::Config(_)
            | Error::Job(_)
            | Error::Image(_)
            | Error::Toml(_)
            | Error::Auth(_) => (Status::Invalid, Some((ErrorCode::Usage, false))),
            Error::Provider(_)
            | Error::Transport(_)
            | Error::Submission(_)
            | Error::NoSubmission(_)
            | Error::ModelOutput(_) => (Status::Error, Some((ErrorCode::Provider, true))),
            Error::Tool(_) => (Status::Error, Some((ErrorCode::ToolProcess, false))),
            Error::Budget(_) => (Status::Error, Some((ErrorCode::Budget, false))),
            Error::Cap(_) => (Status::Error, Some((ErrorCode::Cap, false))),
            Error::StaleEstimate(_) => (Status::Invalid, Some((ErrorCode::StaleEstimate, false))),
            Error::Io(_) | Error::Json(_) => (Status::Error, Some((ErrorCode::Io, false))),
        }
    }

    #[test]
    fn every_error_maps_to_its_contract_status_and_code() {
        let s = || "x".to_string();
        let errors = [
            Error::Cancelled(s()),
            Error::Input(s()),
            Error::ImageLimit(s()),
            Error::Config(s()),
            Error::Job(s()),
            Error::Image(s()),
            Error::Toml(toml::from_str::<toml::Value>("=").unwrap_err()),
            Error::Auth(s()),
            Error::Provider(s()),
            Error::Transport(crate::retry::TransportError {
                status: Some(503),
                retry_after: None,
                message: s(),
                sent: true,
            }),
            Error::Submission(s()),
            Error::NoSubmission(s()),
            Error::ModelOutput(s()),
            Error::Tool(s()),
            Error::Budget(s()),
            Error::Cap(s()),
            Error::StaleEstimate(s()),
            Error::Io(std::io::Error::other("x")),
            Error::Json(serde_json::from_str::<serde_json::Value>("{").unwrap_err()),
        ];
        for e in &errors {
            assert_eq!(status_of(e.class()), expected(e), "{e}");
        }
    }
}
