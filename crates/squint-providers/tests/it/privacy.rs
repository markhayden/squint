//! Task 2.4: a run is refused without consent bound to the profile, a missing key is `error: auth`,
//! a redirect to another host is `error: provider`, credentials are redacted in real request
//! shapes, and every recorded fixture is scanned for paths and image metadata.
#![allow(clippy::unwrap_used)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use squint_core::artifacts::ArtifactStore;
use squint_core::config::Config;
use squint_providers::catalog::Catalog;
use squint_providers::http::{Client, Transport};
use squint_providers::privacy::{FORBIDDEN_MARKERS, consent, credential, scan, scan_fixtures};

fn root() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn config(account: &str) -> Result<Config, squint_core::Error> {
    let text = format!(
        "[provider]\nprofile = \"anthropic\"\nmodel = \"claude-opus-5-5\"\n\n[privacy]\ncloud_calls = true\naccount = \"{account}\"\n\n[output]\ndir = \"out\"\n"
    );
    Config::parse(&text, &[], &|_| None)
}

#[test]
fn privacy_consent_is_bound_to_the_profile() {
    let catalog = Catalog::shipped().unwrap();
    let anthropic = catalog.profile("anthropic").unwrap();
    // Without an account the config itself is refused.
    let err = config("").unwrap_err();
    assert!(err.to_string().contains("account"), "{err}");
    // An account for another provider does not consent to this profile.
    let err = consent(&config("openai:org-abc").unwrap(), "anthropic", anthropic).unwrap_err();
    assert_eq!(err.class(), "resolve");
    assert!(
        err.to_string().contains("needs the prefix `anthropic:`"),
        "{err}"
    );
    assert!(
        !err.to_string().contains("org-abc"),
        "the account id is not echoed: {err}"
    );
    let err = consent(&config("anthropic").unwrap(), "anthropic", anthropic).unwrap_err();
    assert!(err.to_string().contains("anthropic:<account id>"), "{err}");
    let ok = consent(
        &config("anthropic:org-123456").unwrap(),
        "anthropic",
        anthropic,
    )
    .unwrap();
    assert_eq!(ok.profile, "anthropic");
    assert_eq!(ok.account, "anthropic:org-123456");
    assert!(ok.upstream_allow.is_empty());
    // Each shipped profile declares the prefix its accounts carry.
    assert_eq!(
        catalog.profile("google-ai-studio").unwrap().account,
        "google"
    );
    assert_eq!(catalog.profile("openai").unwrap().account, "openai");
}

#[test]
fn privacy_missing_credential_is_a_usage_error_and_the_value_never_prints() {
    let catalog = Catalog::shipped().unwrap();
    let profile = catalog.profile("openai").unwrap();
    let err = credential(profile, &|_| None).unwrap_err();
    assert_eq!(err.class(), "resolve");
    assert!(err.to_string().contains("OPENAI_API_KEY"), "{err}");
    let err = credential(profile, &|_| Some("   ".into())).unwrap_err();
    assert_eq!(err.class(), "resolve");
    let c = credential(profile, &|name| {
        (name == "OPENAI_API_KEY").then(|| "sk-test-7781".to_string())
    })
    .unwrap();
    assert_eq!(c.expose(), "sk-test-7781");
    assert_eq!(c.env(), "OPENAI_API_KEY");
    assert!(!format!("{c:?}").contains("7781") && !c.to_string().contains("7781"));
}

#[test]
fn privacy_redaction_covers_real_request_shapes() {
    let dir = std::env::temp_dir().join(format!("squint-privacy-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let key = "sk-ant-api03-SYNTHETIC/abc+def=";
    let store = ArtifactStore::create(&dir, &[key.to_string()]).unwrap();
    let shapes = [
        format!("x-api-key: {key}\r\nanthropic-version: 2023-06-01"),
        format!("Authorization: Bearer {key}"),
        format!("POST https://generativelanguage.googleapis.com/v1beta/models/gemini-4-argon:generateContent?key={key}"),
        serde_json::json!({ "headers": { "x-goog-api-key": key }, "error": format!("invalid key {key}") }).to_string(),
        format!("error: provider: HTTP 401: {{\"message\":\"{}\"}}", serde_json::to_string(key).unwrap().trim_matches('"')),
    ];
    for shape in &shapes {
        let out = store.redact(shape);
        assert!(!out.contains("SYNTHETIC"), "{out}");
        assert!(out.contains("[redacted]"), "{out}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn privacy_scan_flags_paths_and_metadata_and_runs_over_every_fixture_directory() {
    let dirty = r#"{"image":{"path":"/Users/someone/fixtures/images/a.png"},"meta":"Exif\u0000"}"#;
    let findings = scan(dirty);
    assert!(
        findings.iter().any(|f| f.contains("/Users/"))
            && findings.iter().any(|f| f.contains("Exif")),
        "{findings:?}"
    );
    assert!(scan(r#"{"type":"image","source":{"type":"base64","media_type":"image/jpeg","data":"/9j/4AAQ"}}"#).is_empty());
    assert!(FORBIDDEN_MARKERS.contains(&"GPano"));
    // Every recorded request under fixtures/adapters and fixtures/profiles is clean.
    let findings = scan_fixtures(&root()).unwrap();
    assert!(findings.is_empty(), "{findings:#?}");
    // A planted dirty fixture is found.
    let tmp = std::env::temp_dir().join(format!("squint-fixtures-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("fixtures/adapters/anthropic-messages/first-look")).unwrap();
    std::fs::write(
        tmp.join("fixtures/adapters/anthropic-messages/first-look/request-1.json"),
        dirty,
    )
    .unwrap();
    let findings = scan_fixtures(&tmp).unwrap();
    assert_eq!(findings.len(), 3, "{findings:?}");
    assert!(
        findings[0].starts_with("fixtures/adapters/anthropic-messages/first-look/request-1.json: ")
    );
    let _ = std::fs::remove_dir_all(&tmp);
}

/// A one-thread HTTP server answering a fixed number of requests by path.
fn serve(responses: usize) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for _ in 0..responses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut buf = vec![0u8; 65536];
            let mut read = 0;
            loop {
                let n = stream.read(&mut buf[read..]).unwrap_or(0);
                if n == 0 {
                    break;
                }
                read += n;
                if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&buf[..read]).to_string();
            let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
            let response = match path.as_str() {
                "/moved" => format!("HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:{port}/ok\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
                "/away" => "HTTP/1.1 302 Found\r\nLocation: http://example.invalid:9/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                "/busy" => "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 7\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"error\":\"slow\"}".to_string(),
                _ => "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nrequest-id: req_1\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}".to_string(),
            };
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    port
}

fn hdr(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[tokio::test]
async fn privacy_http_client_follows_same_origin_redirects_only() {
    let port = serve(4);
    let client = Client::new(Duration::from_secs(10)).unwrap();
    let body = serde_json::json!({ "model": "x" });
    let ok = client
        .post_json(
            &format!("http://127.0.0.1:{port}/moved"),
            &hdr(&[("x-api-key", "k")]),
            &body,
        )
        .await
        .unwrap();
    assert_eq!(ok.status, 200);
    assert_eq!(ok.body, "{\"ok\":true}");
    assert_eq!(ok.header("request-id"), Some("req_1"));
    let err = client
        .post_json(&format!("http://127.0.0.1:{port}/away"), &[], &body)
        .await
        .unwrap_err();
    assert_eq!(err.class(), "provider", "{err}");
    assert!(
        err.to_string().contains("another host") && err.to_string().contains("example.invalid"),
        "{err}"
    );
    let busy = client
        .post_json(&format!("http://127.0.0.1:{port}/busy"), &[], &body)
        .await
        .unwrap();
    assert_eq!(busy.status, 429);
    match busy.ok(|_| false).unwrap_err() {
        squint_core::Error::Transport(t) => {
            assert_eq!(t.status, Some(429));
            assert_eq!(t.retry_after, Some(Duration::from_secs(7)));
            assert!(t.retryable());
            assert!(t.message.contains("slow"));
        }
        other => panic!("{other}"),
    }
}

#[tokio::test]
async fn privacy_http_client_reports_a_refused_connection_as_retryable_transport() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let client = Client::new(Duration::from_secs(5)).unwrap();
    let err = client
        .post_json(
            &format!("http://127.0.0.1:{port}/x"),
            &[],
            &serde_json::json!({}),
        )
        .await
        .unwrap_err();
    match err {
        squint_core::Error::Transport(t) => {
            assert_eq!(t.status, None);
            assert!(t.retryable());
            // The connection carried no request: not sent, so it costs nothing (table-manners 4.5).
            assert!(!t.sent && !t.unresolved(), "{t:?}");
            assert!(
                !t.message.contains("127.0.0.1"),
                "no URL in messages: {}",
                t.message
            );
        }
        other => panic!("{other}"),
    }
}
