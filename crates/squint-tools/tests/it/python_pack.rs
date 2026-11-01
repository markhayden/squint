//! Task 5.1: the Rust client against the real Python pack. `echo_region` is called with an image
//! and an aligned frame, both by path, and answers with a valid envelope whose one image arrives
//! as an MCP image block. F4: without the segmenter artifact a pack serving the tools that need it
//! refuses `initialize`, naming the file and the fetch command, and one serving only
//! `echo_region` starts. Needs `uv` and a synced `tools/python` environment
//! (`just python-check`), so it runs with `--ignored`; CI's python job runs it.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use squint_core::frames::Mapping;
use squint_tools::mcp::{Pack, Spawn, Timeouts};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("squint-python-pack-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn png(path: &std::path::Path, width: u32, height: u32, value: u8) {
    let img = image::RgbImage::from_pixel(width, height, image::Rgb([value, value, value]));
    img.save(path).unwrap();
}

#[test]
#[ignore = "needs uv and a synced tools/python environment (just python-check)"]
fn echo_region_cuts_a_box_from_an_aligned_frame_and_returns_it_as_an_image_block() {
    let run_dir = tempdir("run");
    let candidate = run_dir.join("raw_candidate.png");
    let aligned = run_dir.join("aligned_candidate_t1.png");
    png(&candidate, 40, 20, 30);
    png(&aligned, 40, 20, 200);
    let pack = Pack::connect(
        "python",
        Spawn {
            command: PathBuf::from("uv"),
            args: [
                "run",
                "--frozen",
                "--offline",
                "--no-sync",
                "squint-tools",
                "serve",
                "echo_region",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            declared_env: vec![],
            cwd: Some(root().join("tools/python")),
            extra_env: vec![],
        },
        Timeouts {
            startup: Duration::from_secs(60),
            call: Duration::from_secs(30),
            shutdown_grace: Duration::from_millis(500),
        },
        std::env::vars(),
    )
    .unwrap();
    let manifest = &pack.manifests()[0];
    assert_eq!(manifest.name, "echo_region");
    assert_eq!(manifest.frame_args, vec!["frame".to_string()]);

    let handle = |path: &std::path::Path| json!({ "media_type": "image/png", "width": 40, "height": 20, "projection": "flat", "path": path });
    let e = pack
        .call(
            "run-1",
            Some(&run_dir),
            "echo_region",
            json!({
                "image": "candidate",
                "bbox": [0.5, 0.0, 1.0, 0.5],
                "frame": "aligned:candidate:t1",
                "_squint": {
                    "run_id": "run-1",
                    "call_id": "call:1",
                    "images": { "candidate": handle(&candidate) },
                    "frames": {
                        "aligned:candidate:t1": {
                            "root": "candidate",
                            "projection": "flat",
                            "parent": "raw:source",
                            "transform": { "id": "t1", "mapping": { "homography": [1, 0, 0, 0, 1, 0, 0, 0, 1] } },
                            "chain": [{ "frame": "aligned:candidate:t1", "parent": "raw:source" }],
                            "pixels": handle(&aligned)
                        }
                    },
                    "hints": {}
                }
            }),
        )
        .unwrap();
    assert_eq!(e.status, squint_core::tool::Status::Ok, "{}", e.summary);
    assert_eq!(e.regions.len(), 1);
    let [img] = e.images.as_slice() else {
        panic!("one image expected, got {}", e.images.len());
    };
    assert_eq!(img.frame, "crop:call:1:0");
    assert_eq!(img.parent, "aligned:candidate:t1");
    assert_eq!(img.mapping, Mapping::Bbox([0.5, 0.0, 1.0, 0.5]));
    assert_eq!(
        (img.width, img.height, img.media_type.as_str()),
        (20, 10, "image/png")
    );
    // The bytes came as the image block and are the aligned frame's pixels, not the candidate's.
    let bytes =
        squint_core::image::base64_decode(img.base64.as_deref().expect("the block's bytes"))
            .unwrap();
    let cut = image::load_from_memory(&bytes).unwrap().to_rgb8();
    assert_eq!(cut.dimensions(), (20, 10));
    assert_eq!(cut.get_pixel(0, 0).0, [200, 200, 200]);
    pack.shutdown();
}

/// The synced environment's own executable, so the pack sees exactly the `XDG_CACHE_HOME` given
/// (through `uv run` it would also move uv's cache).
fn pack_with_models_in(cache_home: &std::path::Path, args: &[&str]) -> Spawn {
    Spawn {
        command: root().join("tools/python/.venv/bin/squint-tools"),
        args: args.iter().map(|s| s.to_string()).collect(),
        declared_env: vec![],
        cwd: None,
        extra_env: vec![("XDG_CACHE_HOME".into(), cache_home.display().to_string())],
    }
}

fn timeouts() -> Timeouts {
    Timeouts {
        startup: Duration::from_secs(60),
        call: Duration::from_secs(30),
        shutdown_grace: Duration::from_millis(500),
    }
}

#[test]
#[ignore = "needs a synced tools/python environment (just python-check)"]
fn without_the_segmenter_initialize_names_the_missing_file_and_echo_region_alone_starts() {
    let cache_home = tempdir("no-models");
    let artifact = cache_home.join("squint/models/segmenter-v1/eomt-large-ade20k-512.onnx");
    let Err(e) = Pack::connect(
        "python",
        pack_with_models_in(&cache_home, &["serve"]),
        timeouts(),
        std::env::vars(),
    ) else {
        panic!("the pack started without the segmenter");
    };
    let message = e.to_string();
    assert!(
        message.contains("initialize failed: regions and classes need the segmenter artifact")
            && message.contains(&format!("{} is missing", artifact.display()))
            && message.contains("`squint-tools fetch`"),
        "{message}"
    );

    let pack = Pack::connect(
        "python",
        pack_with_models_in(&cache_home, &["serve", "echo_region"]),
        timeouts(),
        std::env::vars(),
    )
    .unwrap();
    let names: Vec<&str> = pack.manifests().iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["echo_region"]);
    pack.shutdown();
}
