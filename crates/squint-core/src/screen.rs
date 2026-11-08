//! Running tools without a model: the order a tool and its prerequisites run in, and the
//! arguments a call gets when no model chose them. Shared by `squint tools call` and the
//! screen a rules profile asks for before the model's first turn (`[screen]`).

use crate::tool::Manifest;

/// The tools named and everything they require, prerequisites first, each once.
pub fn prerequisites<'a>(
    tools: impl IntoIterator<Item = &'a str>,
    manifests: &[Manifest],
) -> Result<Vec<String>, String> {
    fn visit(
        name: &str,
        manifests: &[Manifest],
        seen: &mut Vec<String>,
        path: &mut Vec<String>,
    ) -> Result<(), String> {
        if seen.iter().any(|s| s == name) {
            return Ok(());
        }
        if path.iter().any(|p| p == name) {
            return Err(format!("tool {name:?} requires itself"));
        }
        let m = manifests.iter().find(|m| m.name == name).ok_or_else(|| {
            let known: Vec<&str> = manifests.iter().map(|m| m.name.as_str()).collect();
            format!(
                "no tool {name:?} in this build or the configured packs; known: {}",
                known.join(", ")
            )
        })?;
        path.push(name.to_string());
        for req in &m.requires {
            visit(req, manifests, seen, path)?;
        }
        path.pop();
        seen.push(name.to_string());
        Ok(())
    }
    let mut seen = Vec::new();
    for tool in tools {
        visit(tool, manifests, &mut seen, &mut Vec::new())?;
    }
    Ok(seen)
}

/// `given` with every required frame or image argument it leaves out filled from the argument's
/// description: the first of "aligned", "source" or "candidate" after "usually" (else anywhere)
/// picks the newest aligned frame, the source or the candidate. Image arguments take the image
/// id, frame arguments the raw frame id. Any other missing required argument is the caller's.
pub fn with_defaults(
    manifest: &Manifest,
    mut given: serde_json::Value,
    frames: &[String],
) -> Result<serde_json::Value, String> {
    let schema = &manifest.model_schema;
    let required: Vec<&str> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();
    let Some(obj) = given.as_object_mut() else {
        return Err(format!(
            "{}: arguments must be a JSON object",
            manifest.name
        ));
    };
    for arg in required {
        if obj.contains_key(arg) {
            continue;
        }
        let is_image = manifest.image_args.iter().any(|a| a == arg);
        let is_frame = manifest.frame_args.iter().any(|a| a == arg);
        if !is_image && !is_frame {
            return Err(format!(
                "{} needs {arg:?}; pass it in --args, for example --args '{{\"{arg}\": …}}' ({})",
                manifest.name,
                describe(schema, arg)
            ));
        }
        let text = describe(schema, arg).to_lowercase();
        let hint = text
            .split_once("usually")
            .map_or(text.as_str(), |(_, after)| after);
        let first = ["aligned", "source", "candidate"]
            .into_iter()
            .filter_map(|w| hint.find(w).map(|at| (at, w)))
            .min();
        let value = match first.map(|(_, w)| w) {
            // No aligned frame: align could not register the pair (a strong restyle often
            // defeats it though the framing is unchanged). The raw candidate stands in; a tool
            // that needs one grid refuses a pair whose sizes differ.
            Some("aligned") => frames
                .iter()
                .rev()
                .find(|f| f.starts_with("aligned:candidate:"))
                .cloned()
                .unwrap_or_else(|| "raw:candidate".to_string()),
            Some(image) if is_image => image.to_string(),
            Some(image) => format!("raw:{image}"),
            None => {
                return Err(format!(
                    "{} needs {arg:?} and its description names no default; pass it in --args",
                    manifest.name
                ));
            }
        };
        obj.insert(arg.to_string(), serde_json::Value::String(value));
    }
    Ok(given)
}

fn describe<'a>(schema: &'a serde_json::Value, arg: &str) -> &'a str {
    schema
        .pointer(&format!("/properties/{arg}/description"))
        .and_then(|d| d.as_str())
        .unwrap_or("")
}

/// The words that open the screen's part of the first message.
pub fn preamble(tools: &[String]) -> String {
    format!(
        "Before your first turn the harness ran {} on these images for you. Their results follow, \
         as if you had called them: the regions they found are already on the checklist, and you \
         may cite them. A screen result is a pointer, not a verdict: confirm what it flags with \
         view, and look further where it found nothing but the images give you reason to.",
        tools
            .iter()
            .map(|t| format!("`{t}`"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}
