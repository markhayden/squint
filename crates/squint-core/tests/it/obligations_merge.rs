//! Task 1.4a: merge order, replacement, disabling, provenance and every validation rule.
#![allow(clippy::unwrap_used)]

use squint_core::Error;
use squint_core::job::{ImageSource, ImageSpec, Job, Role};
use squint_core::obligations::{Kind, MergeInput, Obligation, RulesFile, merge, shipped};

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

fn pair_job() -> Job {
    job(
        "source-fidelity",
        &[("candidate", Role::Candidate), ("source", Role::Source)],
    )
}

fn consumer(text: &str) -> RulesFile {
    RulesFile::parse(text, "mine.toml").unwrap()
}

fn merge_with(
    extra: &[RulesFile],
    job: Option<&Job>,
    tools: &[&str],
) -> squint_core::Result<squint_core::obligations::Merged> {
    let names: Vec<String> = extra
        .iter()
        .map(|_| "./rules/mine.toml".to_string())
        .collect();
    let tools: Vec<String> = tools.iter().map(|t| t.to_string()).collect();
    merge(&MergeInput {
        base: "source-fidelity",
        profile: None,
        extra,
        extra_names: &names,
        job,
        enabled_tools: &tools,
    })
}

fn err_contains(r: squint_core::Result<squint_core::obligations::Merged>, needle: &str) {
    match r {
        Err(Error::Config(m)) => assert!(m.contains(needle), "expected {needle:?} in {m:?}"),
        other => panic!("expected a config error containing {needle:?}, got {other:?}"),
    }
}

#[test]
fn base_alone_merges_with_base_provenance() {
    let m = merge_with(&[], Some(&pair_job()), &[]).unwrap();
    let ids: Vec<&str> = m
        .obligations
        .iter()
        .map(|o| o.obligation.id.as_str())
        .collect();
    assert_eq!(ids, ["SF1", "SF2", "SF3", "SF4"]);
    assert!(
        m.obligations
            .iter()
            .all(|o| o.provenance == "base:source-fidelity")
    );
    assert_eq!(m.versions["base:source-fidelity"], 1);
    assert_eq!(
        m.default_evidence_kinds,
        ["change_region", "class_difference", "alignment_failure"]
    );
}

#[test]
fn architecture_profile_replaces_sf1_and_sf3_with_profile_provenance() {
    let m = merge(&MergeInput {
        base: "source-fidelity",
        profile: Some("architecture"),
        extra: &[],
        extra_names: &[],
        job: Some(&pair_job()),
        enabled_tools: &["regions".into()],
    })
    .unwrap();
    let sf1 = m
        .obligations
        .iter()
        .find(|o| o.obligation.id == "SF1")
        .unwrap();
    assert_eq!(sf1.provenance, "profile:architecture");
    assert!(sf1.obligation.hidden_ok);
    assert!(sf1.obligation.text.contains("Permanent structure"));
    let sf3 = m
        .obligations
        .iter()
        .find(|o| o.obligation.id == "SF3")
        .unwrap();
    assert!(sf3.obligation.waives.is_empty());
    let sf2 = m
        .obligations
        .iter()
        .find(|o| o.obligation.id == "SF2")
        .unwrap();
    assert_eq!(
        sf2.provenance, "base:source-fidelity",
        "untouched rules keep base provenance"
    );
    assert!(m.hints.contains_key("regions"));
    assert!(m.warnings.is_empty());
}

#[test]
fn hints_for_a_tool_not_enabled_are_dropped_with_a_warning() {
    let m = merge(&MergeInput {
        base: "source-fidelity",
        profile: Some("architecture"),
        extra: &[],
        extra_names: &[],
        job: Some(&pair_job()),
        enabled_tools: &[],
    })
    .unwrap();
    assert!(!m.hints.contains_key("regions"));
    assert_eq!(m.warnings.len(), 1);
    assert!(m.warnings[0].contains("hints.regions"));
}

#[test]
fn unknown_profile_and_wrong_extends_are_refused() {
    let r = merge(&MergeInput {
        base: "source-fidelity",
        profile: Some("nope"),
        extra: &[],
        extra_names: &[],
        job: None,
        enabled_tools: &[],
    });
    err_contains(r, "no shipped profile");
    let r = merge(&MergeInput {
        base: "nope",
        profile: None,
        extra: &[],
        extra_names: &[],
        job: None,
        enabled_tools: &[],
    });
    err_contains(r, "no shipped base set");
}

#[test]
fn consumer_file_replaces_disables_and_appends() {
    let f = consumer(
        r#"
[rules]
use_case = "source-fidelity"
version = 3

[[rule]]
id = "SF2"
kind = "must_hold"
scope = ["candidate"]
text = "Replaced text for SF2."

[[rule]]
id = "SF4"
kind = "observe"
disabled = true

[[rule]]
id = "MY1"
kind = "must_not"
scope = ["candidate"]
text = "No watermark anywhere in the candidate."
"#,
    );
    let m = merge_with(&[f], Some(&pair_job()), &[]).unwrap();
    let ids: Vec<&str> = m
        .obligations
        .iter()
        .map(|o| o.obligation.id.as_str())
        .collect();
    assert_eq!(ids, ["SF1", "SF2", "SF3", "MY1"]);
    let sf2 = m.get("SF2").unwrap();
    assert_eq!(sf2.text, "Replaced text for SF2.");
    assert_eq!(
        m.obligations
            .iter()
            .find(|o| o.obligation.id == "SF2")
            .unwrap()
            .provenance,
        "file:./rules/mine.toml"
    );
    assert_eq!(m.versions["file:./rules/mine.toml"], 3);
    assert_eq!(m.get("MY1").unwrap().kind, Kind::MustNot);
}

#[test]
fn job_requirements_join_the_merge_last_with_job_provenance() {
    let mut j = pair_job();
    j.requirements.push(Obligation {
        id: "RQ1".into(),
        kind: Kind::Allowed,
        text: "The pool may be extended by one tile.".into(),
        matters: None,
        hidden_ok: false,
        scope: Some(vec!["candidate".into()]),
        evidence: Some(vec!["source".into()]),
        evidence_kinds: None,
        waives: vec!["SF1".into()],
        condition: Some("Only the near edge moves, by at most one tile.".into()),
        disabled: false,
    });
    let m = merge_with(&[], Some(&j), &[]).unwrap();
    let rq = m
        .obligations
        .iter()
        .find(|o| o.obligation.id == "RQ1")
        .unwrap();
    assert_eq!(rq.provenance, "job");
    assert_eq!(rq.obligation.waives, ["SF1"]);
}

#[test]
fn validation_table() {
    let cases: Vec<(&str, &str)> = vec![
        (
            "waives unknown",
            r#"[[rule]]
id = "MY1"
kind = "allowed"
waives = ["ZZ9"]
condition = "c"
text = "t""#,
        ),
        (
            "not mandatory",
            r#"[[rule]]
id = "MY1"
kind = "allowed"
waives = ["SF4"]
condition = "c"
text = "t""#,
        ),
        (
            "no condition",
            r#"[[rule]]
id = "MY1"
kind = "allowed"
waives = ["SF1"]
text = "t""#,
        ),
        (
            "cannot waive",
            r#"[[rule]]
id = "MY1"
kind = "must_hold"
waives = ["SF1"]
text = "t""#,
        ),
        (
            "takes no condition",
            r#"[[rule]]
id = "MY1"
kind = "observe"
condition = "c"
text = "t""#,
        ),
        (
            "only for mandatory",
            r#"[[rule]]
id = "MY1"
kind = "observe"
hidden_ok = true
text = "t""#,
        ),
        (
            "explicit empty scope",
            r#"[[rule]]
id = "MY1"
kind = "must_hold"
scope = []
text = "t""#,
        ),
        (
            "neither a role nor an image id",
            r#"[[rule]]
id = "MY1"
kind = "must_hold"
scope = ["ref:nobody"]
text = "t""#,
        ),
        (
            "has no text",
            r#"[[rule]]
id = "MY1"
kind = "must_hold""#,
        ),
        (
            "two capital letters",
            r#"[[rule]]
id = "my1"
kind = "must_hold"
text = "t""#,
        ),
    ];
    for (needle, rule) in cases {
        let f = consumer(&format!(
            "[rules]\nuse_case = \"source-fidelity\"\nversion = 1\n\n{rule}\n"
        ));
        err_contains(merge_with(&[f], Some(&pair_job()), &[]), needle);
    }
}

#[test]
fn use_case_mismatch_and_missing_required_role_are_refused() {
    let f = consumer("[rules]\nuse_case = \"reality-checks\"\nversion = 1\n");
    err_contains(merge_with(&[f], Some(&pair_job()), &[]), "use case");
    let j = job(
        "reality-checks",
        &[("candidate", Role::Candidate), ("source", Role::Source)],
    );
    err_contains(merge_with(&[], Some(&j), &[]), "does not match");
    let j = job("source-fidelity", &[("candidate", Role::Candidate)]);
    err_contains(
        merge_with(&[], Some(&j), &[]),
        "requires an image with role",
    );
}

#[test]
fn job_requirement_ids_must_start_with_rq() {
    let mut j = pair_job();
    j.requirements.push(Obligation {
        id: "SF9".into(),
        kind: Kind::MustHold,
        text: "t".into(),
        matters: None,
        hidden_ok: false,
        scope: None,
        evidence: None,
        evidence_kinds: None,
        waives: vec![],
        condition: None,
        disabled: false,
    });
    err_contains(merge_with(&[], Some(&j), &[]), "must start with RQ");
}

#[test]
fn shipped_files_are_the_ones_on_disk() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let disk = std::fs::read_to_string(root.join("crates/squint-core/rules/source-fidelity.toml"))
        .unwrap();
    assert_eq!(shipped("source-fidelity").unwrap(), disk);
}
