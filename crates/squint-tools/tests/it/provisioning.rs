//! Task 1.14: a clean-directory install from the built bundle yields a working executable whose
//! version matches the lock; the harness calls `echo_region` through it offline; a tampered digest
//! fails. Needs `uv` and the bundle from `scripts/build-bundle.sh`, so these run with `--ignored`.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::time::Duration;

use serde_json::json;
use squint_core::tool::Status;
use squint_tools::mcp::{Pack, Timeouts};
use squint_tools::provision::{Bundle, Fetch, Installed};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bundle_dir() -> PathBuf {
    let dir = repo().join("tools/python/dist/bundle-0.0.1");
    assert!(
        dir.join("bundle.toml").exists(),
        "no bundle at {}: run scripts/build-bundle.sh first",
        dir.display()
    );
    dir
}

fn tempdir(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("squint-provisioning-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
#[ignore = "needs uv and a built bundle (scripts/build-bundle.sh)"]
fn clean_install_serves_echo_region_offline_with_versions_matching_the_lock() {
    let bundle = Bundle::load(&bundle_dir()).unwrap();
    bundle.verify().unwrap();
    let data_home = tempdir("install");
    // No fetch here: the artifact is 1.2 GB and lives behind the network; the pack's tools that
    // need it report it missing, which is their own test's business.
    let installed = bundle.install(&data_home, &Fetch::Skip).unwrap();
    assert!(installed.executable.is_file());
    assert_eq!(installed.header.version, "0.0.1");

    // The installed executable reports the bundle's version, and the lock pins the same one.
    let out = std::process::Command::new(&installed.executable)
        .arg("--version")
        .env("UV_OFFLINE", "1")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        installed.header.version
    );
    let lock = std::fs::read_to_string(installed.dir.join("uv.lock")).unwrap();
    assert!(
        lock.contains("name = \"squint-tools\"")
            && lock.contains(&format!("version = \"{}\"", installed.header.version)),
        "the lock pins the installed version"
    );
    assert_eq!(
        Installed::locate(&data_home, "squint-tools", "0.0.1")
            .unwrap()
            .dir,
        installed.dir
    );

    // The harness launches it offline and calls the tool through MCP.
    let timeouts = Timeouts {
        startup: Duration::from_secs(60),
        call: Duration::from_secs(30),
        shutdown_grace: Duration::from_secs(2),
    };
    let pack = Pack::connect(
        "python",
        installed.spawn(vec![]),
        timeouts,
        std::env::vars(),
    )
    .unwrap();
    let names: Vec<&str> = pack.manifests().iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["echo_region", "regions", "classes"]);
    let candidate = data_home.join("raw_candidate.png");
    image::RgbImage::from_pixel(8, 6, image::Rgb([90, 90, 90]))
        .save(&candidate)
        .unwrap();
    let envelope = pack
        .call(
            "run-1",
            None,
            "echo_region",
            json!({
                "image": "candidate",
                "bbox": [0.0, 0.0, 0.5, 0.5],
                "_squint": { "run_id": "run-1", "call_id": "call:1", "images": { "candidate": { "media_type": "image/png", "width": 8, "height": 6, "projection": "flat", "path": candidate } }, "frames": {}, "hints": {} },
            }),
        )
        .unwrap();
    assert_eq!(envelope.status, Status::Ok, "{}", envelope.summary);
    assert_eq!(envelope.regions.len(), 1);
    assert_eq!(envelope.images.len(), 1);
    assert_eq!(
        envelope.provenance.lock_digest.as_deref(),
        Some(installed.lock_digest.as_str())
    );
}

#[test]
#[ignore = "needs a built bundle (scripts/build-bundle.sh)"]
fn a_tampered_bundle_is_refused_before_anything_is_installed() {
    let copy = tempdir("tampered").join("bundle");
    std::fs::create_dir_all(&copy).unwrap();
    for entry in std::fs::read_dir(bundle_dir()).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), copy.join(entry.file_name())).unwrap();
    }
    let mut model = std::fs::read_to_string(copy.join("MODEL.md")).unwrap();
    model.push_str("\ntampered\n");
    std::fs::write(copy.join("MODEL.md"), model).unwrap();
    let data_home = tempdir("tampered-home");
    let err = Bundle::load(&copy)
        .unwrap()
        .install(&data_home, &Fetch::Skip)
        .unwrap_err();
    assert_eq!(err.class(), "resolve");
    assert!(err.to_string().contains("digest mismatch"), "{err}");
    assert!(!data_home.join("squint").exists(), "nothing was installed");
}

#[test]
#[ignore = "needs uv, a built bundle and the export's output in tools/python/dist/segmenter"]
fn setup_fetches_the_segmenter_from_a_directory_into_the_models_cache() {
    let export = repo().join("tools/python/dist/segmenter");
    // The export is 1.2 GB and exists only where the export script ran (or the release was
    // downloaded into it); without it there is nothing to fetch from, which is not a failure.
    if !export.join("record.json").is_file() {
        eprintln!(
            "skipped: no export under {} (run the export script, or download the segmenter-v1 release assets there)",
            export.display()
        );
        return;
    }
    let bundle = Bundle::load(&bundle_dir()).unwrap();
    let data_home = tempdir("fetch-home");
    let cache_home = tempdir("fetch-cache");
    let models = cache_home.join("squint/models");
    bundle
        .install(
            &data_home,
            &Fetch::From {
                dir: export,
                models: Some(models.clone()),
            },
        )
        .unwrap();
    let artifact = cache_home.join("squint/models/segmenter-v1/eomt-large-ade20k-512.onnx");
    assert!(artifact.is_file(), "{} was fetched", artifact.display());
    assert_eq!(artifact.metadata().unwrap().len(), 1_264_419_839);
    for n in 0..4 {
        assert!(
            cache_home
                .join(format!("squint/models/segmenter-v1/golden-{n}.npz"))
                .is_file()
        );
    }
    // A tampered copy is refused and nothing of it is kept.
    let tampered = tempdir("fetch-tampered");
    std::fs::write(
        tampered.join("eomt-large-ade20k-512.onnx"),
        b"not the model",
    )
    .unwrap();
    let cache_two = tempdir("fetch-cache-two");
    let err = Bundle::load(&bundle_dir())
        .unwrap()
        .install(
            &tempdir("fetch-home-two"),
            &Fetch::From {
                dir: tampered,
                models: Some(cache_two.join("squint/models")),
            },
        )
        .unwrap_err();
    assert_eq!(err.class(), "resolve");
    assert!(err.to_string().contains("is not the recorded"), "{err}");
    assert!(
        std::fs::read_dir(cache_two.join("squint/models/segmenter-v1"))
            .map(|d| d.count() == 0)
            .unwrap_or(true),
        "nothing kept"
    );
}
