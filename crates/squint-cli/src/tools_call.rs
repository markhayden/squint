//! `squint tools call`: one tool on one pair of images, with no model (docs/pipeline.md).
//!
//! Stateless: every call starts from the images, runs the prerequisites the tool declares
//! (`requires = ["align"]` runs `align` first, in the same call) and writes everything under one
//! directory. Calls go through the harness's own validation and frame bookkeeping, so frames are
//! named and regions mapped onto the original image exactly as in a check.

use std::path::{Path, PathBuf};

use squint_core::run::{ToolCallRecord, ToolSession};
use squint_core::screen::{prerequisites, with_defaults};
use squint_core::tool::{Manifest, Tool, ToolRegistry};

use crate::{
    EXIT_USAGE, JobArgs, SharedPacks, available_tools, connect_packs, load_config, load_job,
};

/// A tool failed or broke its bounds; the caller's mistakes exit 64.
const EXIT_TOOL: i32 = 1;

#[derive(clap::Args)]
pub(crate) struct ToolsCallArgs {
    /// The tool to run, for example `regions`.
    tool: String,
    /// The tool's arguments as JSON, or `@file` to read them from a file. Any frame or image
    /// argument left out is filled from its description ("usually the aligned candidate");
    /// anything else it requires must be given.
    #[arg(long, value_name = "JSON")]
    args: Option<String>,
    /// The directory to write into: the journal, every frame and `result.json`. Defaults to a
    /// fresh `tools-<time>-<tool>` under the configured output directory.
    #[arg(long)]
    out: Option<PathBuf>,
    /// The rules profile whose hints the tools get, for example `architecture` (`none` for the
    /// base rules); as the check actions' `profile`.
    #[arg(long)]
    profile: Option<String>,
    /// Print `result.json` instead of the text summary.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    input: JobArgs,
}

pub(crate) fn run(args: &ToolsCallArgs) -> i32 {
    match call(args) {
        Ok(code) => code,
        Err((code, message)) => {
            eprintln!("squint: {message}");
            code
        }
    }
}

fn usage(message: impl std::fmt::Display) -> (i32, String) {
    (EXIT_USAGE, message.to_string())
}

fn call(args: &ToolsCallArgs) -> Result<i32, (i32, String)> {
    let (mut config, _, _) = load_config(&args.input).map_err(usage)?;
    let job = load_job(&args.input).map_err(usage)?;
    squint_core::contract::CallChoices {
        profile: args.profile.clone(),
        tools: None,
    }
    .apply(&job.use_case, &mut config)
    .map_err(usage)?;
    let given = match args.args.as_deref() {
        None => serde_json::json!({}),
        Some(text) => {
            let text = match text.strip_prefix('@') {
                Some(path) => std::fs::read_to_string(path)
                    .map_err(|e| usage(format!("--args {path}: {e}")))?,
                None => text.to_string(),
            };
            serde_json::from_str(&text).map_err(|e| usage(format!("--args: {e}")))?
        }
    };
    if !given.is_object() {
        return Err(usage("--args must be a JSON object"));
    }

    // Every tool this build and the configured packs offer, whether the config enables it or
    // not; the registry keeps the tool and its prerequisites.
    let mut available: Vec<Box<dyn Tool>> = Vec::new();
    for (pack, _) in connect_packs(&config, &SharedPacks::default()).map_err(usage)? {
        available.extend(pack.tools());
    }
    for tool in available_tools(None).map_err(usage)? {
        if !available
            .iter()
            .any(|t| t.manifest().name == tool.manifest().name)
        {
            available.push(tool);
        }
    }
    let manifests: Vec<Manifest> = available.iter().map(|t| t.manifest().clone()).collect();
    let order = prerequisites([args.tool.as_str()], &manifests).map_err(usage)?;
    let tools = ToolRegistry::new(available, &order, &[]).map_err(usage)?;

    let out = args.out.clone().unwrap_or_else(|| {
        config.output.dir.join(format!(
            "tools-{}-{}",
            squint_core::contract::now_ms(),
            args.tool
        ))
    });
    if out.join("journal.jsonl").exists() {
        return Err(usage(format!(
            "{} already holds a tools call; give a fresh --out",
            out.display()
        )));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| usage(format!("cannot start the runtime: {e}")))?;
    let mut session = ToolSession::open(&config, &job, Path::new("."), &tools, &out)
        .map_err(|e| usage(e.to_string()))?;

    let mut records: Vec<ToolCallRecord> = Vec::new();
    for name in &order {
        let manifest = manifests
            .iter()
            .find(|m| &m.name == name)
            .expect("ordered from the manifests");
        let explicit = if name == &args.tool {
            given.clone()
        } else {
            serde_json::json!({})
        };
        // What ran before a failure stays on record: an `align` that could not register is
        // the reason its dependant had no frame.
        let arguments = match with_defaults(manifest, explicit, &session.frames()) {
            Ok(a) => a,
            Err(e) => {
                write_result(&out, args, &records).map_err(|e| (EXIT_TOOL, e))?;
                return Err(usage(e));
            }
        };
        let record = runtime
            .block_on(session.call(name, arguments))
            .map_err(|e| (EXIT_TOOL, format!("{name}: {e}")))?;
        let refused = record.rejected.clone();
        records.push(record);
        if let Some(reason) = refused {
            write_result(&out, args, &records).map_err(|e| (EXIT_TOOL, e))?;
            return Err(usage(format!("{name}: {reason}")));
        }
    }
    let result = write_result(&out, args, &records).map_err(|e| (EXIT_TOOL, e))?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&result).unwrap_or_default()
        );
    } else {
        print_summary(&out, &records);
    }
    Ok(0)
}

fn write_result(
    out: &Path,
    args: &ToolsCallArgs,
    records: &[ToolCallRecord],
) -> Result<serde_json::Value, String> {
    let result = serde_json::json!({
        "tool": args.tool,
        "dir": std::path::absolute(out).unwrap_or_else(|_| out.to_path_buf()),
        "calls": records,
    });
    let path = out.join("result.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&result).unwrap_or_default(),
    )
    .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(result)
}

fn print_summary(out: &Path, records: &[ToolCallRecord]) {
    let round = |b: &[f64; 4]| format!("[{:.2} {:.2} {:.2} {:.2}]", b[0], b[1], b[2], b[3]);
    for r in records {
        let Some(env) = &r.envelope else { continue };
        println!(
            "{} ({}) {:?}: {}",
            r.tool, r.call_id, env.status, env.summary
        );
        for m in &env.measurements {
            let unit = if m.unit.is_empty() { "" } else { " " };
            println!("  {} = {:.4}{unit}{}", m.name, m.value, m.unit);
        }
        for (region, root) in env.regions.iter().zip(&r.roots) {
            // A box through a homography is a polygon; its bounds are what a reader wants.
            let bounds = |l: &squint_core::report::RootLocation| {
                l.bbox.or_else(|| {
                    let p = l.polygon.as_ref().filter(|p| !p.is_empty())?;
                    let (xs, ys): (Vec<f64>, Vec<f64>) = p.iter().map(|[x, y]| (*x, *y)).unzip();
                    let lo = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
                    let hi = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    Some([lo(&xs), lo(&ys), hi(&xs), hi(&ys)])
                })
            };
            let at = root
                .as_ref()
                .and_then(|l| bounds(l).map(|b| format!("{} {}", l.image, round(&b))))
                .unwrap_or_else(|| "(does not map to an image)".into());
            println!(
                "  {} {:<40} score {:.2}  on {at}",
                region.id, region.label, region.score
            );
        }
        for (frame, path) in &r.images {
            println!("  image {frame}: {}", path.display());
        }
    }
    println!("written to {}", out.join("result.json").display());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(text: &str) -> Manifest {
        Manifest::from_toml(text).expect("manifest")
    }

    fn regions() -> Manifest {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/tools/regions.toml"
        ))
        .expect("fixture");
        manifest(&text)
    }

    #[test]
    fn prerequisites_come_first_and_unknown_tools_are_named() {
        let builtins = squint_tools::builtin::manifests();
        let mut all = builtins.clone();
        all.push(regions());
        assert_eq!(
            prerequisites(["regions"], &all).unwrap(),
            ["align", "regions"]
        );
        assert_eq!(prerequisites(["view"], &all).unwrap(), ["view"]);
        let err = prerequisites(["outline"], &all).unwrap_err();
        assert!(err.contains("outline") && err.contains("align"), "{err}");
    }

    #[test]
    fn frame_and_image_arguments_default_from_their_descriptions() {
        let builtins = squint_tools::builtin::manifests();
        let get = |n: &str| builtins.iter().find(|m| m.name == n).unwrap().clone();
        let frames = vec![
            "raw:candidate".to_string(),
            "raw:source".to_string(),
            "aligned:candidate:t1".to_string(),
        ];
        let align = with_defaults(&get("align"), serde_json::json!({}), &frames).unwrap();
        assert_eq!(
            align,
            serde_json::json!({ "image": "candidate", "against": "source" })
        );
        let change = with_defaults(&get("change_map"), serde_json::json!({}), &frames).unwrap();
        assert_eq!(
            change,
            serde_json::json!({ "frame": "raw:source", "against": "aligned:candidate:t1" })
        );
        // Given arguments are kept; a required non-frame argument is the caller's to give.
        let view = with_defaults(
            &get("view"),
            serde_json::json!({ "frame": "raw:source" }),
            &frames,
        )
        .unwrap();
        assert_eq!(view, serde_json::json!({ "frame": "raw:source" }));
        // Without an aligned frame (align refused), the raw candidate stands in.
        let raw_only = vec!["raw:candidate".to_string(), "raw:source".to_string()];
        let unaligned =
            with_defaults(&get("change_map"), serde_json::json!({}), &raw_only).unwrap();
        assert_eq!(unaligned["against"], "raw:candidate");
        let err = with_defaults(&get("resize"), serde_json::json!({}), &frames).unwrap_err();
        assert!(err.contains("long_edge"), "{err}");
    }
}
