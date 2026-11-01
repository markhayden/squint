//! The arm C matrix as it is run: for every arm the matrix names, the system prompt built from
//! the arm's real tool selection (the built-ins it enables, under the arm's projections) differs
//! from arm A's only in the Tools section, so a flip between the arms is the tools' doing.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use squint_core::job::Job;
use squint_core::obligations::{MergeInput, merge, resolve};
use squint_core::render::{system_prompt, tools_section};
use squint_core::tool::ToolRegistry;
use squint_eval::admissions::Admissions;
use squint_eval::arms::tools_for;
use squint_eval::matrix::{Arm, Matrix};
use squint_tools::builtin;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn prompt_for(arm: &Arm, job: &Job) -> (String, String) {
    let selection = tools_for(arm, &Admissions::default(), &job.use_case).unwrap();
    let registry = ToolRegistry::new(builtin::all(), &selection.enabled, &[])
        .unwrap()
        .with_projections(&selection.projections)
        .unwrap();
    let merged = merge(&MergeInput {
        base: &job.use_case,
        profile: None,
        extra: &[],
        extra_names: &[],
        job: Some(job),
        enabled_tools: &selection.enabled,
    })
    .unwrap();
    let resolved = resolve(&merged, job).unwrap();
    (
        system_prompt(&resolved, job, registry.manifests()),
        tools_section(registry.manifests()),
    )
}

#[test]
fn arm_c_prompts_differ_from_arm_a_only_in_the_tools_section() {
    let matrix = Matrix::parse(
        &std::fs::read_to_string(repo_root().join("evalsets/matrix/arm-c.toml")).unwrap(),
    )
    .unwrap();
    assert!(
        matrix.matrix.arms.contains(&Arm::C),
        "{:?}",
        matrix.matrix.arms
    );
    let job = Job::load(
        &repo_root().join("fixtures/evalsets/source-fidelity/pairs-v1/items/pair-01/job.toml"),
    )
    .unwrap();
    let (a, a_tools) = prompt_for(&Arm::A, &job);
    for arm in &matrix.matrix.arms {
        let (prompt, tools) = prompt_for(arm, &job);
        if *arm == Arm::A {
            continue;
        }
        assert_ne!(prompt, a, "arm {arm} enables tools");
        assert!(
            tools.contains("- view (") && tools.contains("- align ("),
            "{tools}"
        );
        assert_eq!(
            prompt.replace(&tools, ""),
            a.replace(&a_tools, ""),
            "arm {arm}: everything but the Tools section is identical to arm A"
        );
    }
}
