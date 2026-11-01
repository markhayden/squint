//! Shared by the runner and gate tests: fake factories over the scripted provider and tool, and
//! synthetic sealed sets built from the fixture image pairs.
#![allow(dead_code, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use squint_core::config::Config;
use squint_core::provider::Provider;
use squint_core::run::Cancel;
use squint_core::tool::{Tool, ToolRegistry};
use squint_eval::arms::ToolSelection;
use squint_eval::matrix::{Cell, Matrix};
use squint_eval::runner::{Factories, Plan};
use squint_testkit::{ScriptedProvider, ScriptedTool};

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-eval-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Picks the provider script of a decision from its 1-based index in run order and its cell.
pub type Chooser = Box<dyn Fn(usize, &Cell) -> PathBuf + Send + Sync>;

/// A fake provider per decision from a script, counting decisions and cancelling after `n`.
pub struct Fake {
    pub script: PathBuf,
    pub choose: Option<Chooser>,
    pub decisions: AtomicUsize,
    pub cancel_after: Option<(usize, Cancel)>,
    pub tool_envelopes: Mutex<Option<PathBuf>>,
}

impl Fake {
    pub fn new(script: &str) -> Fake {
        Fake {
            script: root().join("fixtures/scripts").join(script),
            choose: None,
            decisions: AtomicUsize::new(0),
            cancel_after: None,
            tool_envelopes: Mutex::new(None),
        }
    }

    /// A fake whose script depends on the decision, with the `fake_regions` tool available.
    pub fn choosing(choose: Chooser) -> Fake {
        let mut f = Fake::new("held.json");
        f.choose = Some(choose);
        *f.tool_envelopes.lock().unwrap() =
            Some(root().join("fixtures/tool-scripts/regions-25.json"));
        f
    }
}

impl Factories for Fake {
    fn provider(&self, cell: &Cell, _config: &Config) -> squint_eval::Result<Box<dyn Provider>> {
        let n = self.decisions.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((after, cancel)) = &self.cancel_after
            && n > *after
        {
            cancel.cancel();
        }
        let script = match &self.choose {
            Some(choose) => choose(n, cell),
            None => self.script.clone(),
        };
        Ok(Box::new(ScriptedProvider::from_path(&script).unwrap()))
    }

    fn tools(
        &self,
        selection: &ToolSelection,
        _config: &Config,
    ) -> squint_eval::Result<ToolRegistry> {
        let mut available: Vec<Box<dyn Tool>> = Vec::new();
        if let Some(path) = self.tool_envelopes.lock().unwrap().clone() {
            let text = std::fs::read_to_string(path).unwrap();
            available.push(Box::new(ScriptedTool::fake_regions(
                serde_json::from_str(&text).unwrap(),
            )));
        }
        // Arm C's `view` and `align` arrive in M4; until then the fake registers the enabled
        // tools it has and runs the control arm without them.
        let enabled: Vec<String> = selection
            .enabled
            .iter()
            .filter(|name| available.iter().any(|t| t.manifest().name == **name))
            .cloned()
            .collect();
        let projections = selection
            .projections
            .iter()
            .filter(|(t, _)| enabled.contains(t))
            .map(|(t, p)| (t.clone(), p.clone()))
            .collect();
        Ok(ToolRegistry::new(available, &enabled, &[])
            .unwrap()
            .with_projections(&projections)
            .unwrap())
    }
}

/// A plan from a matrix text, the matrix's config read from the repository root and given a
/// cache directory so that bypassing it is observable.
pub fn plan_from_text(
    eval_dir: &Path,
    evalsets_root: &Path,
    matrix_text: &str,
    cache_dir: &Path,
) -> Plan {
    let matrix = Matrix::parse(matrix_text).unwrap();
    let config_text = std::fs::read_to_string(root().join(&matrix.matrix.config)).unwrap()
        + &format!("\n[cache]\ndir = {:?}\n", cache_dir.display().to_string());
    Plan {
        eval_dir: eval_dir.to_path_buf(),
        matrix,
        matrix_text: matrix_text.to_string(),
        config_text,
        evalsets_root: evalsets_root.to_path_buf(),
        admissions: None,
        harness_version: "0.1.0 (test)".into(),
        allow_fake: true,
    }
}

pub fn plan(eval_dir: &Path, matrix_file: &str) -> Plan {
    let text = std::fs::read_to_string(root().join("fixtures/matrix").join(matrix_file)).unwrap();
    plan_from_text(
        eval_dir,
        &root().join("fixtures/evalsets"),
        &text,
        &eval_dir.join("cache"),
    )
}

/// A synthetic sealed-ready set from the fixture pairs (see `squint_testkit::synthetic_set`).
pub fn build_set(
    evalsets_root: &Path,
    id: &str,
    split: &str,
    prefix: &str,
    families: usize,
) -> PathBuf {
    squint_testkit::synthetic_set(
        evalsets_root,
        &root().join("fixtures/evalsets/source-fidelity/pairs-v1/items"),
        id,
        split,
        prefix,
        families,
    )
    .unwrap()
}
