//! A scripted fake tool: returns the next canned envelope on each call, so checklist growth and
//! tool-turn handling can be driven offline.

use std::collections::VecDeque;
use std::sync::Mutex;

use squint_core::tool::{Envelope, Manifest, SquintContext, Tool};

/// The fake region finder: evidence kind `change_region`, one image argument.
pub const FAKE_REGIONS_MANIFEST: &str = include_str!("../fixtures/fake_regions.toml");

pub struct ScriptedTool {
    manifest: Manifest,
    envelopes: Mutex<VecDeque<Envelope>>,
    delay: std::time::Duration,
    /// Every call's arguments and context, for tests that assert what the harness injected.
    pub calls: Mutex<Vec<(serde_json::Value, SquintContext)>>,
}

impl ScriptedTool {
    /// The fake regions tool with a script of envelopes to return in order.
    pub fn fake_regions(envelopes: Vec<Envelope>) -> ScriptedTool {
        let manifest =
            Manifest::from_toml(FAKE_REGIONS_MANIFEST).expect("fake_regions manifest is valid");
        ScriptedTool {
            manifest,
            envelopes: Mutex::new(envelopes.into()),
            delay: std::time::Duration::ZERO,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Every call sleeps this long first, for cancellation tests.
    pub fn with_delay(mut self, delay: std::time::Duration) -> ScriptedTool {
        self.delay = delay;
        self
    }

    /// Loads envelopes from a JSON array file.
    pub fn fake_regions_from_path(path: &std::path::Path) -> crate::Result<ScriptedTool> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| crate::Error::Script(format!("cannot read {}: {e}", path.display())))?;
        let envelopes: Vec<Envelope> = serde_json::from_str(&text)
            .map_err(|e| crate::Error::Script(format!("{}: {e}", path.display())))?;
        Ok(ScriptedTool::fake_regions(envelopes))
    }
}

impl Tool for ScriptedTool {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn call(&self, args: &serde_json::Value, ctx: &SquintContext) -> squint_core::Result<Envelope> {
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        if let Ok(mut calls) = self.calls.lock() {
            calls.push((args.clone(), ctx.clone()));
        }
        let next = self.envelopes.lock().ok().and_then(|mut e| e.pop_front());
        next.ok_or_else(|| squint_core::Error::Tool("scripted tool: script exhausted".into()))
    }
}

/// What a manifest's image and frame arguments fail to tell the model, one line per argument
/// (empty when nothing is missing). Every `image_args` and `frame_args` property's description
/// must name both forms the harness accepts: an image id (`candidate`) and a raw frame id
/// (`raw:candidate`). Models that saw only one form sent the other and lost a turn to
/// `invalid_args` (the arm C run, 2026-10-28).
pub fn image_argument_gaps(manifest: &Manifest) -> Vec<String> {
    let mut gaps = Vec::new();
    for arg in manifest.image_args.iter().chain(&manifest.frame_args) {
        let description = manifest.model_schema["properties"][arg.as_str()]["description"]
            .as_str()
            .unwrap_or("");
        if !description.contains("image id") {
            gaps.push(format!("{}.{arg}: names no image id", manifest.name));
        }
        if !description.contains("raw:") {
            gaps.push(format!(
                "{}.{arg}: names no raw frame id such as raw:candidate",
                manifest.name
            ));
        }
    }
    gaps
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_description(description: &str) -> Manifest {
        let text = FAKE_REGIONS_MANIFEST.replace(
            "description = \"The image id to find changes in.\"",
            &format!("description = {description:?}"),
        );
        Manifest::from_toml(&text).expect("valid manifest")
    }

    #[test]
    fn an_image_argument_must_name_both_the_image_id_and_the_raw_frame() {
        let both = with_description("The image: its image id (candidate) or raw:candidate.");
        assert!(image_argument_gaps(&both).is_empty());
        assert_eq!(
            image_argument_gaps(&with_description("The image id to find changes in.")),
            ["fake_regions.image: names no raw frame id such as raw:candidate"]
        );
        assert_eq!(
            image_argument_gaps(&with_description("A frame such as raw:candidate.")),
            ["fake_regions.image: names no image id"]
        );
        assert_eq!(
            image_argument_gaps(&with_description("The image.")).len(),
            2
        );
    }
}
