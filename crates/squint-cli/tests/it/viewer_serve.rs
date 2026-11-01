//! Task 3.6 at the binary: `report view` serves the embedded viewer, `report.json` and the run
//! artifacts of a decision directory on loopback, and nothing else. The devtools check of the
//! page itself (no console errors, loopback-only requests, the crop-frame overlay at its mapped
//! position) is recorded in docs/viewer.md.
//!
//! Task 9.9: both are renders a host can embed (table-manners 3.2). `report view` prints only its
//! URL, frames for the one origin it is given, and exits 0 when its stdin closes or on SIGTERM;
//! `report export` writes one self-contained file outside the decision directory and prints only
//! its absolute path; neither changes a byte of the decision directory.
#![cfg(feature = "testkit")]
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, ExitStatus, Stdio};
use std::time::Duration;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Kills the server when the test ends, panic or not, unless it was waited for.
struct Serving(Option<Child>);

impl Serving {
    fn child(&mut self) -> &mut Child {
        self.0.as_mut().unwrap()
    }
}

impl Drop for Serving {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn get(url: &str, path: &str) -> (u16, String, Vec<u8>) {
    let host = url.trim_start_matches("http://").trim_end_matches('/');
    let mut s = TcpStream::connect(host).unwrap();
    write!(
        s,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).unwrap();
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let status: u16 = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, head, raw[split + 4..].to_vec())
}

/// Runs a scripted check with a tool call that returned a crop frame and returns its decision
/// directory, under a fresh output directory named for `tag`.
fn decision(tag: &str) -> (PathBuf, PathBuf) {
    let out = std::env::temp_dir().join(format!("squint-viewer-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/fake-tool.toml",
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            "fixtures/scripts/one-call-crop-finding.json",
            "--tool-script",
            "fixtures/tool-scripts/crop-image.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        matches!(o.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let dir = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    (out, dir)
}

/// Starts `report view <dir> --port 0 <extra>` with its stdin held open, reads its first stdout
/// line, which must be exactly the URL, and returns the server, the URL and the rest of stdout.
fn view(dir: &Path, extra: &[&str]) -> (Serving, String, BufReader<ChildStdout>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_squint"))
        .args(["report", "view", dir.to_str().unwrap(), "--port", "0"])
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut first = String::new();
    stdout.read_line(&mut first).unwrap();
    let url = first
        .strip_suffix('\n')
        .unwrap_or_else(|| panic!("no URL line: {first:?}"));
    let port = url
        .strip_prefix("http://127.0.0.1:")
        .and_then(|p| p.strip_suffix('/'))
        .unwrap_or_else(|| panic!("the first line is not the URL alone: {first:?}"));
    assert!(port.parse::<u16>().unwrap() > 0, "{first:?}");
    (Serving(Some(child)), url.to_string(), stdout)
}

/// Waits for the server to exit on its own, up to a deadline only a hung process reaches.
fn exits(mut serving: Serving) -> ExitStatus {
    let mut child = serving.0.take().unwrap();
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait().unwrap());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| panic!("report view (pid {pid}) did not exit"))
}

/// Every file under `dir` by its relative path, with its bytes, and every directory.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    fn walk(root: &Path, at: &Path, into: &mut BTreeMap<PathBuf, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(at).unwrap() {
            let path = entry.unwrap().path();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            if path.is_dir() {
                into.insert(rel, None);
                walk(root, &path, into);
            } else {
                into.insert(rel, Some(std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(dir, dir, &mut files);
    files
}

/// The `Content-Security-Policy` of a response head.
fn policy(head: &str) -> &str {
    head.lines()
        .find_map(|l| l.strip_prefix("Content-Security-Policy: "))
        .unwrap_or_else(|| panic!("no policy in {head}"))
}

#[test]
fn report_view_prints_only_its_url_frames_for_one_origin_and_exits_0_when_stdin_closes() {
    let (out, dir) = decision("frame");
    let before = snapshot(&dir);
    let (mut serving, url, mut stdout) = view(&dir, &["--frame-ancestor", "http://127.0.0.1:4801"]);
    for path in [
        "/",
        "/viewer.js",
        "/report.json",
        "/run-1/journal.jsonl",
        "/nothing",
    ] {
        let (_, head, _) = get(&url, path);
        let policy = policy(&head);
        assert_eq!(
            policy,
            "default-src 'self'; img-src 'self' data:; frame-ancestors http://127.0.0.1:4801",
            "{path}"
        );
        let ancestors = policy.split("frame-ancestors ").nth(1).unwrap();
        assert_eq!(
            ancestors.split_whitespace().collect::<Vec<_>>(),
            ["http://127.0.0.1:4801"],
            "exactly the one origin, never loopback in general"
        );
    }
    // Closing stdin ends it with 0, and it printed nothing after the URL across the requests.
    drop(serving.child().stdin.take());
    let status = exits(serving);
    assert_eq!(status.code(), Some(0));
    let mut rest = String::new();
    stdout.read_to_string(&mut rest).unwrap();
    assert_eq!(rest, "", "stdout is the URL line and nothing else");
    assert_eq!(
        snapshot(&dir),
        before,
        "viewing changed the decision directory"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[cfg(unix)]
#[test]
fn report_view_exits_0_on_sigterm_and_sigint() {
    let (out, dir) = decision("signal");
    for signal in ["-TERM", "-INT"] {
        let (mut serving, url, mut stdout) = view(&dir, &[]);
        assert_eq!(get(&url, "/").0, 200);
        assert!(
            Command::new("kill")
                .args([signal, &serving.child().id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        // Its stdin is still open: only the signal ends it.
        let status = exits(serving);
        assert_eq!(status.code(), Some(0), "{signal}");
        let mut rest = String::new();
        stdout.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "", "{signal}");
    }
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn report_view_refuses_anything_but_one_origin_with_64() {
    let (out, dir) = decision("origin");
    for bad in [
        "*",
        "'self'",
        "'none'",
        "127.0.0.1:4801",
        "ftp://127.0.0.1:4801",
        "http://127.0.0.1:4801/",
        "http://127.0.0.1:4801/report",
        "http://*.example.com",
        "http://a;script-src *",
        "http://a http://b",
        "http://a:65536",
    ] {
        let o = Command::new(env!("CARGO_BIN_EXE_squint"))
            .args(["report", "view", dir.to_str().unwrap(), "--port", "0"])
            .args(["--frame-ancestor", bad])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(64), "{bad:?}");
        assert!(o.stdout.is_empty(), "{bad:?}");
    }
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn report_export_writes_one_self_contained_file_outside_the_run_and_prints_only_its_path() {
    let (out, dir) = decision("export");
    let cache = out.join("cache");
    let before = snapshot(&dir);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .env("XDG_CACHE_HOME", &cache)
        .args(["report", "export", dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let stdout = String::from_utf8(o.stdout).unwrap();
    let path = PathBuf::from(
        stdout
            .strip_suffix('\n')
            .filter(|p| !p.contains('\n'))
            .unwrap_or_else(|| panic!("not one line: {stdout:?}")),
    );
    assert!(path.is_absolute(), "{}", path.display());
    let run_id = dir.file_name().unwrap().to_str().unwrap();
    assert_eq!(
        path,
        cache.join("squint/reports").join(format!("{run_id}.html")),
        "the default is the cache, named for the run id"
    );
    assert!(!path.starts_with(dir.canonicalize().unwrap()));
    // Alone in its directory, and needing nothing beside it: every src and href is inline.
    let beside: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(beside, [path.file_name().unwrap()]);
    let page = std::fs::read_to_string(&path).unwrap();
    assert!(page.contains("globalThis.SQUINT_BUNDLE = "));
    for attr in ["src=", "href="] {
        for (i, _) in page.match_indices(attr) {
            let value = page[i + attr.len()..].trim_start_matches(['"', '\'']);
            assert!(
                value.starts_with("data:"),
                "a reference to another file: {}",
                page[i..].chars().take(60).collect::<String>()
            );
        }
    }
    assert!(!page.contains("<link rel=\"stylesheet\""));

    // Viewing too: the run directory is byte-identical after both renders.
    let (mut serving, url, _) = view(&dir, &[]);
    for path in ["/", "/report.json", "/run-1/journal.jsonl"] {
        assert_eq!(get(&url, path).0, 200, "{path}");
    }
    drop(serving.child().stdin.take());
    assert_eq!(exits(serving).code(), Some(0));
    assert_eq!(
        snapshot(&dir),
        before,
        "a render changed the decision directory"
    );

    // An --out inside the decision directory is refused before anything is written.
    for inside in [dir.join("report.html"), dir.join("run-1/page.html")] {
        let o = Command::new(env!("CARGO_BIN_EXE_squint"))
            .args(["report", "export", dir.to_str().unwrap(), "--out"])
            .arg(&inside)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(64), "{}", inside.display());
        assert!(o.stdout.is_empty());
    }
    assert_eq!(snapshot(&dir), before);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn report_view_serves_the_viewer_the_report_and_only_the_run_artifacts() {
    // A decision with a tool call that returned a crop frame.
    let out = std::env::temp_dir().join(format!("squint-viewer-serve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .current_dir(repo_root())
        .args([
            "check",
            "--config",
            "fixtures/config/fake-tool.toml",
            "--job",
            "fixtures/jobs/pair.toml",
            "--provider-script",
            "fixtures/scripts/one-call-crop-finding.json",
            "--tool-script",
            "fixtures/tool-scripts/crop-image.json",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        matches!(o.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let dir = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let report = std::fs::read(dir.join("report.json")).unwrap();
    let r: serde_json::Value = serde_json::from_slice(&report).unwrap();
    assert!(r["runs"][0]["frames"]["crop:call:1:0"]["mapping"]["bbox"].is_array());
    // The finding in the crop frame is mapped to raw coordinates of the candidate, which is what
    // the viewer draws on the root image.
    let root = &r["runs"][0]["findings"][0]["root"];
    assert_eq!(root["image"], "candidate");
    assert_eq!(root["bbox"], serde_json::json!([0.1, 0.1, 0.3, 0.3]));

    let (serving, url, _stdout) = view(&dir, &[]);
    let _serving = serving;

    let (status, head, body) = get(&url, "/");
    assert_eq!(status, 200);
    assert!(head.contains("text/html"), "{head}");
    assert!(String::from_utf8_lossy(&body).contains("<title>squint report</title>"));
    let (status, head, body) = get(&url, "/viewer.js");
    assert_eq!(status, 200);
    assert!(head.contains("text/javascript"), "{head}");
    let js = String::from_utf8_lossy(&body).to_string();
    assert!(js.contains("SUPPORTED_REPORT_VERSIONS = [1]"));
    assert!(
        js.contains("is not supported by this viewer"),
        "the refusal branch for other versions"
    );
    let (status, head, geometry) = get(&url, "/geometry.js");
    assert_eq!(status, 200);
    assert!(head.contains("text/javascript"), "{head}");
    let geometry = String::from_utf8_lossy(&geometry).to_string();
    assert!(geometry.contains("squintGeometry"));
    let (status, head, timeline) = get(&url, "/timeline.js");
    assert_eq!(status, 200);
    assert!(head.contains("text/javascript"), "{head}");
    let timeline = String::from_utf8_lossy(&timeline).to_string();
    assert!(timeline.contains("squintTimeline"));
    let (status, _, css) = get(&url, "/viewer.css");
    assert_eq!(status, 200);
    // Nothing in the page reaches off the server that served it: no absolute URL anywhere but
    // the SVG namespace, no stylesheet import, no preconnect.
    let page = String::from_utf8_lossy(&get(&url, "/").2).to_string();
    let css = String::from_utf8_lossy(&css).to_string();
    for (name, text) in [
        ("index.html", &page),
        ("viewer.js", &js),
        ("geometry.js", &geometry),
        ("timeline.js", &timeline),
        ("viewer.css", &css),
    ] {
        for (i, _) in text.match_indices("http") {
            let hit = &text[i..(i + 30).min(text.len())];
            assert!(
                hit.starts_with("http://www.w3.org/2000/svg"),
                "{name}: {hit}"
            );
        }
        assert!(
            !text.contains("@import") && !text.contains("preconnect"),
            "{name}"
        );
    }
    let (status, head, body) = get(&url, "/report.json");
    assert_eq!(status, 200);
    assert!(head.contains("application/json"), "{head}");
    assert_eq!(body, report, "the report is served byte for byte");

    // Artifacts: an image the model saw and the tool envelope, typed by their bytes.
    let sent = r["runs"][0]["images_sent"].as_object().unwrap();
    let candidate = sent["raw:candidate"]["sha256"].as_str().unwrap();
    let (status, head, body) = get(&url, &format!("/run-1/artifacts/{candidate}"));
    assert_eq!(status, 200);
    assert!(head.contains("image/"), "{head}");
    assert_eq!(
        body,
        std::fs::read(dir.join("run-1/artifacts").join(candidate)).unwrap()
    );
    let envelope = r["runs"][0]["tool_calls"][0]["envelope"]["sha256"]
        .as_str()
        .unwrap();
    let (status, head, _) = get(&url, &format!("/run-1/artifacts/{envelope}"));
    assert_eq!(status, 200);
    assert!(head.contains("application/json"), "{head}");

    // The run's journal, for the timeline.
    let (status, _, body) = get(&url, "/run-1/journal.jsonl");
    assert_eq!(status, 200);
    assert_eq!(
        body,
        std::fs::read(dir.join("run-1/journal.jsonl")).unwrap()
    );

    // Nothing else: traversal, other files of the directory, unknown artifacts, other methods.
    for path in [
        "/run-1/artifacts/../../report.json",
        "/run-1/../journal.jsonl",
        "/run-x/journal.jsonl",
        "/run-1/journal.jsonl.bak",
        "/run-1/frames/raw_candidate.png",
        "/decision.json",
        "/../Cargo.toml",
        &format!("/run-1/artifacts/{}", "0".repeat(64)),
        "/run-1/artifacts/short",
    ] {
        assert_eq!(get(&url, path).0, 404, "{path}");
    }
    let host = url.trim_start_matches("http://").trim_end_matches('/');
    let mut s = TcpStream::connect(host).unwrap();
    write!(
        s,
        "POST / HTTP/1.1\r\nHost: {host}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 405"), "{raw}");
    assert!(
        head.contains("Connection: close") && head.contains("nosniff"),
        "{head}"
    );
    assert!(
        head.contains(
            "Content-Security-Policy: default-src 'self'; img-src 'self' data:; frame-ancestors 'none'\r\n"
        ),
        "with no --frame-ancestor no page may frame the viewer: {head}"
    );
    // A request for another host name (DNS rebinding) is refused before any route.
    let mut s = TcpStream::connect(host).unwrap();
    write!(
        s,
        "GET /report.json HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 421"), "{raw}");
    let mut s = TcpStream::connect(host).unwrap();
    write!(s, "GET /report.json HTTP/1.1\r\nConnection: close\r\n\r\n").unwrap();
    let mut raw = String::new();
    s.read_to_string(&mut raw).unwrap();
    assert!(raw.starts_with("HTTP/1.1 421"), "no Host at all: {raw}");

    // A directory without a report is a usage error.
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .args(["report", "view", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(64));

    // The export: one page carrying the same files, nothing loaded from anywhere.
    let page_path = out.join("exported.html");
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .args([
            "report",
            "export",
            dir.to_str().unwrap(),
            "--out",
            page_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let exported = std::fs::read_to_string(&page_path).unwrap();
    for asset in ["viewer.js", "geometry.js", "timeline.js", "viewer.css"] {
        assert!(
            !exported.contains(&format!("\"{asset}\"")),
            "{asset} is inlined, not referenced"
        );
    }
    // Save as PDF: a button that prints, and print styles that keep the page whole on paper.
    assert!(
        exported.contains(r#"<button id="print""#),
        "the page offers Save as PDF"
    );
    assert!(
        exported.contains("@media print"),
        "the page carries print styles"
    );
    let start =
        exported.find("globalThis.SQUINT_BUNDLE = ").unwrap() + "globalThis.SQUINT_BUNDLE = ".len();
    let end = start + exported[start..].find(";</script>").unwrap();
    let bundle: serde_json::Value = serde_json::from_str(&exported[start..end]).unwrap();
    let files = bundle["files"].as_object().unwrap();
    assert_eq!(
        files["report.json"]["text"].as_str().unwrap().as_bytes(),
        report.as_slice()
    );
    assert!(files.contains_key("run-1/journal.jsonl"));
    assert_eq!(
        files[&format!("run-1/artifacts/{candidate}")]["type"]
            .as_str()
            .unwrap()
            .split('/')
            .next(),
        Some("image")
    );
    let image = squint_core::image::base64_decode(
        files[&format!("run-1/artifacts/{candidate}")]["base64"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        image,
        std::fs::read(dir.join("run-1/artifacts").join(candidate)).unwrap()
    );
    let artifacts = std::fs::read_dir(dir.join("run-1/artifacts"))
        .unwrap()
        .count();
    assert_eq!(
        files
            .keys()
            .filter(|k| k.starts_with("run-1/artifacts/"))
            .count(),
        artifacts,
        "every artifact is embedded"
    );
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        format!("{}\n", page_path.display()),
        "the only stdout is the page's absolute path"
    );
    let o = Command::new(env!("CARGO_BIN_EXE_squint"))
        .args(["report", "export", out.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(64), "no report.json is a usage error");
}
