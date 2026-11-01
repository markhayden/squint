//! Task 4.2b: the viewer's sphere geometry (`viewer/geometry.js`) agrees with the Rust reference
//! (`squint_core::frames::sphere`) on the stored footprints: a box in a view, a view across the
//! seam, a view holding a pole, a polygon in a view and a `view` location's own footprint. The
//! JavaScript runs under `node` when it is on the path; without it the Rust side alone is checked
//! against the fixture and the comparison is reported as skipped.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::process::Command;

use squint_core::frames::aligned;
use squint_core::frames::sphere::{self, View};
use squint_core::frames::{Location, LocationKind};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn location(
    kind: LocationKind,
    bbox: Option<[f64; 4]>,
    polygon: Option<Vec<[f64; 2]>>,
) -> Location {
    Location {
        frame: "crop:call:1:0".into(),
        kind,
        bbox,
        polygon,
        yaw: None,
        pitch: None,
        fov: None,
    }
}

/// An aligned frame's mapping to check: a homography or a rotation.
enum Through {
    Homography([f64; 9]),
    Rotation([f64; 9]),
}

fn aligned_cases() -> Vec<(&'static str, Through, Location)> {
    let (s, c) = (20f64).to_radians().sin_cos();
    vec![
        (
            "box on a homography-aligned frame",
            Through::Homography([1.02, 0.01, -0.05, -0.01, 0.98, 0.03, 0.001, -0.002, 1.0]),
            location(LocationKind::Bbox, Some([0.3, 0.3, 0.6, 0.7]), None),
        ),
        (
            "whole aligned frame clipped at the image edge",
            Through::Homography([1.0, 0.0, 0.1, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]),
            location(LocationKind::WholeImage, None, None),
        ),
        (
            "box on a rotation-aligned panorama",
            Through::Rotation([c, -s, 0.0, s, c, 0.0, 0.0, 0.0, 1.0]),
            location(LocationKind::Bbox, Some([0.7, 0.45, 0.8, 0.55]), None),
        ),
    ]
}

fn cases() -> Vec<(&'static str, View, Location)> {
    vec![
        (
            "box in a view",
            View {
                yaw: 45.0,
                pitch: 0.0,
                fov: 60.0,
            },
            location(LocationKind::Bbox, Some([0.4, 0.4, 0.6, 0.6]), None),
        ),
        (
            "view across the seam",
            View {
                yaw: -178.0,
                pitch: 0.0,
                fov: 60.0,
            },
            location(LocationKind::WholeImage, None, None),
        ),
        (
            "view holding the north pole",
            View {
                yaw: 0.0,
                pitch: 80.0,
                fov: 60.0,
            },
            location(LocationKind::WholeImage, None, None),
        ),
        (
            "triangle in a view",
            View {
                yaw: 120.0,
                pitch: -30.0,
                fov: 90.0,
            },
            location(
                LocationKind::Polygon,
                None,
                Some(vec![[0.2, 0.2], [0.8, 0.3], [0.5, 0.9]]),
            ),
        ),
        (
            "view location footprint",
            View {
                yaw: -60.0,
                pitch: 15.0,
                fov: 40.0,
            },
            location(LocationKind::WholeImage, None, None),
        ),
        (
            "box below the pole in a pole-holding view",
            View {
                yaw: 0.0,
                pitch: 80.0,
                fov: 60.0,
            },
            location(LocationKind::Bbox, Some([0.4, 0.6, 0.6, 0.8]), None),
        ),
        (
            "box at the top of a yaw-0 view crossing the seam behind the camera",
            View {
                yaw: 0.0,
                pitch: 60.0,
                fov: 120.0,
            },
            location(LocationKind::Bbox, Some([0.3, 0.0, 0.7, 0.1]), None),
        ),
    ]
}

fn expected_through(through: &Through, loc: &Location) -> serde_json::Value {
    let (mapped, clipped) = match through {
        Through::Homography(h) => aligned::map_through_homography(loc, h, "raw:candidate").unwrap(),
        Through::Rotation(r) => aligned::map_through_rotation(loc, r, "raw:candidate").unwrap(),
    };
    serde_json::json!({
        "kind": serde_json::to_value(mapped.kind).unwrap(),
        "bbox": mapped.bbox,
        "polygon": mapped.polygon,
        "clipped": clipped,
    })
}

fn expected(view: &View, loc: &Location) -> serde_json::Value {
    let (mapped, clipped) = sphere::map_through_view(loc, view, "raw:pano").unwrap();
    serde_json::json!({
        "kind": serde_json::to_value(mapped.kind).unwrap(),
        "bbox": mapped.bbox,
        "polygon": mapped.polygon,
        "clipped": clipped,
    })
}

fn close(a: &serde_json::Value, b: &serde_json::Value, tolerance: f64, path: &str) {
    match (a, b) {
        (serde_json::Value::Number(x), serde_json::Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            assert!((x - y).abs() <= tolerance, "{path}: {x} vs {y}");
        }
        (serde_json::Value::Array(x), serde_json::Value::Array(y)) => {
            assert_eq!(x.len(), y.len(), "{path}: lengths");
            for (i, (p, q)) in x.iter().zip(y.iter()).enumerate() {
                close(p, q, tolerance, &format!("{path}[{i}]"));
            }
        }
        (serde_json::Value::Object(x), serde_json::Value::Object(y)) => {
            for (k, v) in x {
                close(
                    v,
                    y.get(k).unwrap_or(&serde_json::Value::Null),
                    tolerance,
                    &format!("{path}.{k}"),
                );
            }
        }
        _ => assert_eq!(a, b, "{path}"),
    }
}

#[test]
fn the_viewer_geometry_matches_the_rust_reference_on_the_stored_footprints() {
    let fixture = repo_root().join("fixtures/render/view-footprints.json");
    let mut computed: Vec<serde_json::Value> = cases()
        .iter()
        .map(|(name, view, loc)| {
            serde_json::json!({
                "name": name,
                "view": { "yaw": view.yaw, "pitch": view.pitch, "fov": view.fov },
                "location": loc,
                "expected": expected(view, loc),
            })
        })
        .collect();
    for (name, through, loc) in aligned_cases() {
        let mapping = match &through {
            Through::Homography(h) => serde_json::json!({ "homography": h }),
            Through::Rotation(r) => serde_json::json!({ "rotation": r }),
        };
        computed.push(serde_json::json!({
            "name": name,
            "mapping": mapping,
            "location": loc,
            "expected": expected_through(&through, &loc),
        }));
    }
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        std::fs::write(
            &fixture,
            serde_json::to_string_pretty(&computed).unwrap() + "\n",
        )
        .unwrap();
    }
    let stored: Vec<serde_json::Value> =
        serde_json::from_str(&std::fs::read_to_string(&fixture).unwrap()).unwrap();
    assert_eq!(stored.len(), computed.len());
    for (s, c) in stored.iter().zip(computed.iter()) {
        close(
            &c["expected"],
            &s["expected"],
            1e-12,
            s["name"].as_str().unwrap(),
        );
    }

    let Ok(node) = which_node() else {
        eprintln!("node is not on the path; the JavaScript side of the geometry check was skipped");
        return;
    };
    let script =
        std::env::temp_dir().join(format!("squint-viewer-geometry-{}.js", std::process::id()));
    std::fs::write(
        &script,
        r#"
const fs = require('fs');
require(process.argv[2]);
const g = globalThis.squintGeometry;
const cases = JSON.parse(fs.readFileSync(process.argv[3], 'utf8'));
const out = cases.map(c => {
  const loc = { kind: c.location.kind, bbox: c.location.bbox, polygon: c.location.polygon };
  const r = c.view ? g.mapThroughView(loc, c.view)
    : c.mapping.homography ? g.mapThroughHomography(loc, c.mapping.homography)
    : g.mapThroughRotation(loc, c.mapping.rotation);
  return { kind: r.kind, bbox: r.bbox || null, polygon: r.polygon || null, clipped: r.clipped };
});
process.stdout.write(JSON.stringify(out));
"#,
    )
    .unwrap();
    let o = Command::new(node)
        .arg(&script)
        .arg(repo_root().join("crates/squint-cli/viewer/geometry.js"))
        .arg(&fixture)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let js: Vec<serde_json::Value> = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(js.len(), stored.len());
    for (j, s) in js.iter().zip(stored.iter()) {
        close(j, &s["expected"], 1e-9, s["name"].as_str().unwrap());
    }
    let _ = std::fs::remove_file(script);
}

fn which_node() -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    std::env::split_paths(&path)
        .map(|d| d.join("node"))
        .find(|p| p.is_file())
        .ok_or(())
}
