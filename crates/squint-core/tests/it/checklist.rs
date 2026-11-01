//! Task 1.5b: static items (per instance and per reference), evidence binding by kind and root
//! image, drops, the evidence budget, ordering and overlap cross-references.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::Error;
use squint_core::checklist::Checklist;
use squint_core::frames::{Location, LocationKind, Registry};
use squint_core::job::{Job, Projection, Role};
use squint_core::obligations::{MergeInput, Resolved, merge, resolve};
use squint_core::tool::envelope::{Envelope, Provenance, Region, Status};
use squint_core::tool::manifest::Manifest;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn resolved(job: &Job) -> Resolved {
    let merged = merge(&MergeInput {
        base: &job.use_case,
        profile: None,
        extra: &[],
        extra_names: &[],
        job: Some(job),
        enabled_tools: &[],
    })
    .unwrap();
    resolve(&merged, job).unwrap()
}

fn images(job: &Job) -> Vec<(String, Role)> {
    job.images.iter().map(|i| (i.id.clone(), i.role)).collect()
}

fn registry(job: &Job) -> Registry {
    let mut r = Registry::new();
    for i in &job.images {
        r.add_raw(&i.id, 64, 48, Projection::Flat);
    }
    r
}

fn manifest() -> Manifest {
    Manifest::from_toml(
        &std::fs::read_to_string(root().join("fixtures/tools/regions.toml")).unwrap(),
    )
    .unwrap()
}

fn region(id: &str, frame: &str, b: [f64; 4]) -> Region {
    Region {
        id: id.into(),
        location: Location {
            frame: frame.into(),
            kind: LocationKind::Bbox,
            bbox: Some(b),
            polygon: None,
            yaw: None,
            pitch: None,
            fov: None,
        },
        label: format!("region {id}"),
        score: 0.9,
    }
}

fn envelope(regions: Vec<Region>) -> Envelope {
    Envelope {
        envelope_version: 1,
        status: Status::Ok,
        summary: "regions".into(),
        measurements: vec![],
        regions,
        images: vec![],
        transforms: vec![],
        provenance: Provenance {
            tool: "regions".into(),
            version: "1.0.0".into(),
            duration_ms: 1,
            lock_digest: None,
        },
    }
}

#[test]
fn pip_job_yields_instance_items_and_reference_items_with_dependencies() {
    let job = Job::load(&root().join("fixtures/jobs/pip.toml")).unwrap();
    let r = resolved(&job);
    let c = Checklist::build(&r, &images(&job)).unwrap();
    let ids: Vec<&str> = c.ids().collect();
    assert_eq!(
        ids,
        [
            "chk:ob:RC1@candidate",
            "chk:ob:RC2@candidate",
            "chk:ob:RC3@candidate",
            "chk:ob:RQ1@candidate",
            "chk:ref:ref:pip",
            "chk:ref:ref:bridge"
        ]
    );
    let pip = c.get("chk:ref:ref:pip").unwrap();
    assert_eq!(
        pip.instance_ids,
        [
            "RC1@candidate",
            "RC2@candidate",
            "RC3@candidate",
            "RQ1@candidate"
        ]
    );
    let bridge = c.get("chk:ref:ref:bridge").unwrap();
    assert_eq!(
        bridge.instance_ids,
        ["RC1@candidate", "RC2@candidate", "RC3@candidate"],
        "RQ1 consults only ref:pip"
    );
    assert_eq!(
        c.get("chk:ob:RQ1@candidate").unwrap().root_image.as_deref(),
        Some("candidate")
    );
}

#[test]
fn three_regions_returned_one_dropped_two_presented() {
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let r = resolved(&job);
    let mut c = Checklist::build(&r, &images(&job)).unwrap();
    let reg = registry(&job);
    // Two regions on the candidate (owned by SF1, whose default kinds include change_region) and
    // one on the source, which no mandatory instance targets.
    let mut env = envelope(vec![
        region("r1", "raw:candidate", [0.1, 0.1, 0.3, 0.3]),
        region("r2", "raw:source", [0.1, 0.1, 0.3, 0.3]),
        region("r3", "raw:candidate", [0.6, 0.6, 0.9, 0.9]),
    ]);
    let ingest = c
        .ingest("call:3", &mut env, &manifest(), &r, &reg, 24)
        .unwrap();
    assert_eq!(ingest.added, ["chk:ev:3:0", "chk:ev:3:1"]);
    assert_eq!(ingest.dropped.len(), 1);
    assert_eq!(ingest.dropped[0].region_id, "r2");
    assert!(ingest.dropped[0].reason.contains("source"));
    assert_eq!(
        env.regions.len(),
        2,
        "the dropped region is removed before presentation"
    );
    let ev0 = c.get("chk:ev:3:0").unwrap();
    assert_eq!(
        ev0.instance_ids,
        ["SF1@candidate"],
        "SF2 accepts only alignment_failure"
    );
    assert_eq!(ev0.root_image.as_deref(), Some("candidate"));
    assert_eq!(ev0.label.as_deref(), Some("region r1"));
    c.check_invariant().unwrap();
}

#[test]
fn the_evidence_budget_is_enforced_before_presentation() {
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let r = resolved(&job);
    let mut c = Checklist::build(&r, &images(&job)).unwrap();
    let reg = registry(&job);
    let first: Vec<Region> = (0..23)
        .map(|i| {
            region(
                &format!("r{i}"),
                "raw:candidate",
                [0.01 * i as f64, 0.0, 0.01 * i as f64 + 0.005, 0.01],
            )
        })
        .collect();
    let mut env = envelope(first);
    c.ingest("call:1", &mut env, &manifest(), &r, &reg, 24)
        .unwrap();
    assert_eq!(
        c.items
            .iter()
            .filter(|i| i.id.starts_with("chk:ev:"))
            .count(),
        23
    );
    let mut env2 = envelope(vec![
        region("a", "raw:candidate", [0.5, 0.5, 0.6, 0.6]),
        region("b", "raw:candidate", [0.7, 0.7, 0.8, 0.8]),
    ]);
    let err = c
        .ingest("call:2", &mut env2, &manifest(), &r, &reg, 24)
        .unwrap_err();
    assert!(
        matches!(err, Error::Budget(ref m) if m.contains("25")),
        "{err}"
    );
    assert_eq!(err.class(), "budget");
    assert_eq!(
        c.items
            .iter()
            .filter(|i| i.id.starts_with("chk:ev:"))
            .count(),
        23,
        "nothing from call:2 was added"
    );
}

#[test]
fn overlapping_regions_keep_their_own_items_and_cross_reference() {
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let r = resolved(&job);
    let mut c = Checklist::build(&r, &images(&job)).unwrap();
    let reg = registry(&job);
    let mut env = envelope(vec![
        region("r1", "raw:candidate", [0.10, 0.10, 0.50, 0.50]),
        region("r2", "raw:candidate", [0.11, 0.11, 0.50, 0.50]),
    ]);
    c.ingest("call:4", &mut env, &manifest(), &r, &reg, 24)
        .unwrap();
    let a = c.get("chk:ev:4:0").unwrap();
    let b = c.get("chk:ev:4:1").unwrap();
    assert_eq!(a.overlaps, ["chk:ev:4:1"]);
    assert_eq!(b.overlaps, ["chk:ev:4:0"]);
    let text = c.render(&r.instances);
    assert!(
        text.contains("chk:ev:4:1 [region r2]") && text.contains("(overlaps chk:ev:4:0)"),
        "{text}"
    );
}

#[test]
fn a_tool_without_an_evidence_kind_has_all_its_regions_dropped() {
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let r = resolved(&job);
    let mut c = Checklist::build(&r, &images(&job)).unwrap();
    let reg = registry(&job);
    let text = std::fs::read_to_string(root().join("fixtures/tools/regions.toml"))
        .unwrap()
        .replace("evidence_kind = \"change_region\"\n", "");
    let m = Manifest::from_toml(&text).unwrap();
    let mut env = envelope(vec![region("r1", "raw:candidate", [0.1, 0.1, 0.3, 0.3])]);
    let ingest = c.ingest("call:5", &mut env, &m, &r, &reg, 24).unwrap();
    assert!(ingest.added.is_empty());
    assert_eq!(ingest.dropped.len(), 1);
    assert!(env.regions.is_empty());
}

#[test]
fn an_invalid_region_location_is_dropped_not_fatal() {
    let job = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let r = resolved(&job);
    let mut c = Checklist::build(&r, &images(&job)).unwrap();
    let reg = registry(&job);
    let mut env = envelope(vec![region("r1", "raw:nowhere", [0.1, 0.1, 0.3, 0.3])]);
    let ingest = c
        .ingest("call:6", &mut env, &manifest(), &r, &reg, 24)
        .unwrap();
    assert_eq!(ingest.dropped.len(), 1);
    assert!(ingest.dropped[0].reason.contains("location invalid"));
}
