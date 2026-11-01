//! Task 1.4b: resolution of obligations to instances (targets, evidence, defaults, errors), the
//! rendered Obligations section (snapshots), and every shipped set resolving against a fitting job.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::Error;
use squint_core::job::{ImageSource, ImageSpec, Job, Role};
use squint_core::obligations::{Kind, MergeInput, Obligation, SHIPPED_BASES, merge, resolve};
use squint_core::render;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn job(use_case: &str, roles: &[(&str, Role)]) -> Job {
    Job {
        use_case: use_case.into(),
        brief: None,
        requirements: Vec::new(),
        images: roles
            .iter()
            .map(|(id, role)| ImageSpec {
                id: (*id).into(),
                role: *role,
                label: None,
                source: ImageSource::Path {
                    path: "x.png".into(),
                    sha256: None,
                },
                projection: None,
                expect: None,
            })
            .collect(),
    }
}

fn resolved_for(
    base: &str,
    profile: Option<&str>,
    job: &Job,
) -> squint_core::Result<squint_core::obligations::Resolved> {
    let merged = merge(&MergeInput {
        base,
        profile,
        extra: &[],
        extra_names: &[],
        job: Some(job),
        enabled_tools: &[],
    })?;
    resolve(&merged, job)
}

fn err_contains(r: squint_core::Result<squint_core::obligations::Resolved>, needle: &str) {
    match r {
        Err(Error::Config(m)) => assert!(m.contains(needle), "expected {needle:?} in {m:?}"),
        other => panic!("expected a config error containing {needle:?}, got {other:?}"),
    }
}

fn snapshot(name: &str, actual: &str) {
    let path = root().join("fixtures/render").join(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::write(&path, actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_default();
    assert_eq!(
        expected, actual,
        "snapshot {name} differs; run with UPDATE_SNAPSHOTS=1 to accept"
    );
}

#[test]
fn pair_job_resolves_sf1_and_sf2_on_the_candidate_consulting_the_source() {
    let j = job(
        "source-fidelity",
        &[("candidate", Role::Candidate), ("source", Role::Source)],
    );
    let r = resolved_for("source-fidelity", None, &j).unwrap();
    let ids: Vec<String> = r.instances.iter().map(|i| i.id()).collect();
    assert_eq!(ids, ["SF1@candidate", "SF2@candidate"]);
    assert_eq!(r.instances[0].evidence, ["source"]);
    assert_eq!(
        r.instances[0].evidence_kinds,
        ["change_region", "class_difference", "alignment_failure"],
        "set default"
    );
    assert_eq!(
        r.instances[1].evidence_kinds,
        ["alignment_failure"],
        "explicit kinds win"
    );
    assert_eq!(r.exemptions.len(), 1);
    assert_eq!(
        r.exemptions[0].targets,
        ["candidate"],
        "default scope is the candidate"
    );
    assert_eq!(r.observations.len(), 1);
}

#[test]
fn roles_expand_to_every_image_with_that_role_in_job_order() {
    let mut j = job(
        "reference-consistency",
        &[
            ("candidate", Role::Candidate),
            ("ref:a", Role::Reference),
            ("ref:b", Role::Reference),
        ],
    );
    j.requirements.push(Obligation {
        id: "RQ1".into(),
        kind: Kind::MustHold,
        text: "Each reference is intact.".into(),
        matters: None,
        hidden_ok: false,
        scope: Some(vec!["reference".into()]),
        evidence: Some(vec!["candidate".into()]),
        evidence_kinds: None,
        waives: vec![],
        condition: None,
        disabled: false,
    });
    let r = resolved_for("reference-consistency", None, &j).unwrap();
    let rq: Vec<String> = r
        .instances
        .iter()
        .filter(|i| i.obligation_id == "RQ1")
        .map(|i| i.id())
        .collect();
    assert_eq!(rq, ["RQ1@ref:a", "RQ1@ref:b"]);
    let rc1 = r.instance("RC1@candidate").unwrap();
    assert_eq!(
        rc1.evidence,
        ["ref:a", "ref:b"],
        "role 'reference' expands in job order"
    );
}

#[test]
fn default_evidence_is_every_source_and_reference() {
    let mut j = job(
        "reality-checks",
        &[("candidate", Role::Candidate), ("ref:x", Role::Reference)],
    );
    // RE1 has no evidence list, so it consults the reference and the reference counts as consulted.
    j.requirements.clear();
    let r = resolved_for("reality-checks", None, &j).unwrap();
    assert_eq!(r.instance("RE1@candidate").unwrap().evidence, ["ref:x"]);
}

#[test]
fn a_mandatory_obligation_with_no_target_is_an_error() {
    let mut j = job("reality-checks", &[("candidate", Role::Candidate)]);
    j.requirements.push(Obligation {
        id: "RQ1".into(),
        kind: Kind::MustHold,
        text: "References are intact.".into(),
        matters: None,
        hidden_ok: false,
        scope: Some(vec!["reference".into()]),
        evidence: None,
        evidence_kinds: None,
        waives: vec![],
        condition: None,
        disabled: false,
    });
    err_contains(
        resolved_for("reality-checks", None, &j),
        "resolves to no target",
    );
}

#[test]
fn a_check_with_no_mandatory_instance_is_an_error() {
    let j = job("reality-checks", &[("candidate", Role::Candidate)]);
    let text = "[rules]\nuse_case = \"reality-checks\"\nversion = 1\n".to_string()
        + &["RE1", "RE2", "RE3", "RE4"]
            .iter()
            .map(|id| format!("[[rule]]\nid = \"{id}\"\nkind = \"must_not\"\ndisabled = true\n"))
            .collect::<String>();
    let f = squint_core::obligations::RulesFile::parse(&text, "mine").unwrap();
    let merged = merge(&MergeInput {
        base: "reality-checks",
        profile: None,
        extra: &[f],
        extra_names: &["mine".into()],
        job: Some(&j),
        enabled_tools: &[],
    })
    .unwrap();
    err_contains(resolve(&merged, &j), "no mandatory instance");
}

#[test]
fn an_unconsulted_reference_is_an_error() {
    let mut j = job(
        "source-fidelity",
        &[
            ("candidate", Role::Candidate),
            ("source", Role::Source),
            ("ref:stray", Role::Reference),
        ],
    );
    j.requirements.clear();
    // SF1 and SF2 both consult only the source, so ref:stray is consulted by nothing.
    err_contains(
        resolved_for("source-fidelity", None, &j),
        "consulted by no mandatory obligation",
    );
}

#[test]
fn exemption_applies_only_to_its_waived_obligations_on_its_targets() {
    let j = Job::load(&root().join("fixtures/jobs/pip.toml")).unwrap();
    let r = resolved_for("reference-consistency", None, &j).unwrap();
    let rq2 = r.exemptions.iter().find(|e| e.id == "RQ2").unwrap();
    assert!(rq2.applies_to("RC3", "candidate"));
    assert!(!rq2.applies_to("RC1", "candidate"));
    assert!(!rq2.applies_to("RC3", "ref:pip"));
    let rq1 = r.instance("RQ1@candidate").unwrap();
    assert_eq!(rq1.evidence, ["ref:pip"]);
    assert_eq!(rq1.provenance, "job");
}

#[test]
fn every_shipped_base_resolves_against_a_fitting_job() {
    for base in SHIPPED_BASES {
        let roles: &[(&str, Role)] = match *base {
            "source-fidelity" => &[("candidate", Role::Candidate), ("source", Role::Source)],
            "reference-consistency" | "product-fidelity" => {
                &[("candidate", Role::Candidate), ("ref:x", Role::Reference)]
            }
            _ => &[("candidate", Role::Candidate)],
        };
        let j = job(base, roles);
        let r = resolved_for(base, None, &j).unwrap_or_else(|e| panic!("{base}: {e}"));
        assert!(!r.instances.is_empty(), "{base}");
        assert!(
            r.exemptions.iter().all(|e| e.waives.is_empty()),
            "{base}: shipped exemptions waive nothing"
        );
    }
}

#[test]
fn obligations_section_snapshot_pip() {
    let j = Job::load(&root().join("fixtures/jobs/pip.toml")).unwrap();
    let r = resolved_for("reference-consistency", None, &j).unwrap();
    let text = render::obligations_section(&r, &j);
    assert!(text.contains("Rules:") && text.contains("Job requirements:"));
    assert!(
        text.contains("RQ2 (allowed; on candidate)")
            && text.contains("May waive: RC3.")
            && text.contains("Condition:")
    );
    assert!(text.contains("<<<\nA golden retriever") && text.contains("never an instruction"));
    snapshot("pip.txt", &text);
}

#[test]
fn obligations_section_snapshot_architecture() {
    let j = Job::load(&root().join("fixtures/jobs/pair.toml")).unwrap();
    let r = resolved_for("source-fidelity", Some("architecture"), &j).unwrap();
    let text = render::obligations_section(&r, &j);
    assert!(
        text.contains("Permanent structure"),
        "profile text replaces the base"
    );
    assert!(
        text.contains("say `hidden`; that is recorded, not unsure"),
        "hidden_ok rendered"
    );
    assert!(
        text.contains("waives nothing and is never an `allowed_by`"),
        "SF3 with waives = []"
    );
    assert!(text.contains("SF4 (observe; on candidate)") && text.contains("`observed` findings"));
    assert!(
        text.contains("allowed_change"),
        "the assertion instruction is present"
    );
    snapshot("architecture.txt", &text);
    assert_eq!(text, render::obligations_section(&r, &j), "byte-stable");
}

// ---- task 4.1: the Tools section is byte-stable -----------------------------------------------

#[test]
fn tools_section_snapshot() {
    let regions = squint_core::tool::Manifest::from_toml(
        &std::fs::read_to_string(root().join("fixtures/tools/regions.toml")).unwrap(),
    )
    .unwrap();
    let fake = squint_core::tool::Manifest::from_toml(squint_testkit::tool::FAKE_REGIONS_MANIFEST)
        .unwrap();
    let text = render::tools_section([&fake, &regions].into_iter());
    assert!(text.starts_with("\nTools. ") && text.contains("- regions (Expensive, about 25s)"));
    assert!(text.contains("Requires: align"), "{text}");
    snapshot("tools.txt", &text);
    assert_eq!(
        text,
        render::tools_section([&fake, &regions].into_iter()),
        "byte-stable"
    );
}
