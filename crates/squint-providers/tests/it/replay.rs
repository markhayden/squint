//! Task 2.5a: the replay harness. A hand-written recording replays; a request differing only in a
//! volatile header passes; dropping a thinking block from the echoed assistant turn fails with the
//! pointer; the recorder writes exchanges with secrets stripped that the player then replays.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use squint_providers::http::{Response, Transport, TransportFuture};
use squint_providers::replay::{
    Exchange, Player, REDACTED, RecordedRequest, RecordedResponse, Recorder, Volatile,
    first_difference, load,
};

const URL: &str = "https://api.anthropic.com/v1/messages";

fn headers(key: &str) -> Vec<(String, String)> {
    vec![
        ("x-api-key".into(), key.into()),
        ("anthropic-version".into(), "2023-06-01".into()),
        ("content-type".into(), "application/json".into()),
    ]
}

fn thinking() -> Value {
    json!({ "type": "thinking", "thinking": "", "signature": "EqQBCgIYAhIM1gbcDa9GJwZA2b" })
}

fn request_one() -> Value {
    json!({
        "model": "claude-opus-5-5", "max_tokens": 1000,
        "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }] }]
    })
}

fn request_two() -> Value {
    json!({
        "model": "claude-opus-5-5", "max_tokens": 1000,
        "messages": [
            { "role": "user", "content": [{ "type": "text", "text": "hi" }] },
            { "role": "assistant", "content": [thinking(), { "type": "text", "text": "ok" }] },
            { "role": "user", "content": [{ "type": "text", "text": "again" }] }
        ]
    })
}

fn exchange(
    request: Value,
    status: u16,
    response_headers: &[(&str, &str)],
    response: Value,
) -> Exchange {
    let mut h: BTreeMap<String, String> = BTreeMap::new();
    for (k, v) in headers(REDACTED) {
        h.insert(k, v);
    }
    Exchange {
        request: RecordedRequest {
            url: URL.into(),
            headers: h,
            body: request,
        },
        response: RecordedResponse {
            status,
            headers: response_headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            body: response,
        },
    }
}

fn recording() -> Vec<Exchange> {
    vec![
        exchange(
            request_one(),
            200,
            &[("request-id", "req_1")],
            json!({ "id": "msg_1", "content": [thinking(), { "type": "text", "text": "ok" }], "stop_reason": "end_turn", "usage": { "input_tokens": 3, "output_tokens": 2 } }),
        ),
        exchange(
            request_two(),
            429,
            &[("retry-after", "3")],
            json!({ "type": "error", "error": { "type": "rate_limit_error", "message": "slow down" } }),
        ),
    ]
}

fn write(dir: &std::path::Path, exchanges: &[Exchange]) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).unwrap();
    for (i, e) in exchanges.iter().enumerate() {
        std::fs::write(
            dir.join(format!("{:02}.json", i + 1)),
            serde_json::to_string_pretty(e).unwrap(),
        )
        .unwrap();
    }
}

fn tmp(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("squint-replay-{name}-{}", std::process::id()))
}

#[tokio::test]
async fn replay_serves_a_recording_and_ignores_volatile_headers() {
    let dir = tmp("basic");
    write(&dir, &recording());
    let player = Player::load(&dir, Volatile::common()).unwrap();
    assert_eq!(player.remaining(), 2);
    // A different key and an extra volatile header still match.
    let mut h = headers("sk-ant-live-key");
    h.push(("user-agent".into(), "squint/test".into()));
    let r = player.post_json(URL, &h, &request_one()).await.unwrap();
    assert_eq!(r.status, 200);
    assert_eq!(r.header("request-id"), Some("req_1"));
    assert!(r.body.contains("EqQBCgIYAhIM1gbcDa9GJwZA2b"));
    // The recorded 429 comes back as a response; `ok` turns it into the transport error.
    let r = player.post_json(URL, &h, &request_two()).await.unwrap();
    assert_eq!(r.status, 429);
    let err = r
        .ok(squint_providers::anthropic::names_no_cause)
        .unwrap_err();
    match err {
        squint_core::Error::Transport(t) => {
            assert_eq!(t.status, Some(429));
            assert_eq!(t.retry_after, Some(std::time::Duration::from_secs(3)));
        }
        other => panic!("{other}"),
    }
    assert_eq!(player.remaining(), 0);
    let err = player.post_json(URL, &h, &request_one()).await.unwrap_err();
    assert!(err.to_string().contains("holds 2 exchanges"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn replay_refuses_a_directory_with_no_exchanges() {
    let dir = tmp("empty");
    write(&dir, &[]);
    std::fs::write(dir.join("expected.json"), "{}").unwrap();
    let err = match Player::load(&dir, Volatile::common()) {
        Ok(_) => panic!("an empty recording loaded"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("holds no exchanges"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn replay_fails_when_a_thinking_block_is_dropped_or_a_real_header_changes() {
    let player = Player::new(recording(), Volatile::common());
    player
        .post_json(URL, &headers("k"), &request_one())
        .await
        .unwrap();
    let mut without_thinking = request_two();
    without_thinking["messages"][1]["content"] = json!([{ "type": "text", "text": "ok" }]);
    let err = player
        .post_json(URL, &headers("k"), &without_thinking)
        .await
        .unwrap_err();
    assert_eq!(err.class(), "provider");
    assert!(
        err.to_string().contains("exchange 2")
            && err
                .to_string()
                .contains("/messages/1/content: recorded 2 items, sent 1"),
        "{err}"
    );
    // An altered signature is caught at the field.
    let player = Player::new(recording(), Volatile::common());
    player
        .post_json(URL, &headers("k"), &request_one())
        .await
        .unwrap();
    let mut altered = request_two();
    altered["messages"][1]["content"][0]["signature"] = json!("tampered");
    let err = player
        .post_json(URL, &headers("k"), &altered)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("/messages/1/content/0/signature"),
        "{err}"
    );
    // A non-volatile header that changes fails too; so does the URL.
    let player = Player::new(recording(), Volatile::common());
    let mut h = headers("k");
    h[1].1 = "2024-01-01".into();
    let err = player.post_json(URL, &h, &request_one()).await.unwrap_err();
    assert!(
        err.to_string().contains("headers differ") && err.to_string().contains("anthropic-version"),
        "{err}"
    );
    let player = Player::new(recording(), Volatile::common());
    let err = player
        .post_json(
            "https://example.com/v1/messages",
            &headers("k"),
            &request_one(),
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("url differs"), "{err}");
    // Declared volatile body pointers are ignored.
    let volatile = Volatile {
        body_pointers: vec!["/metadata/user_id".into()],
        ..Volatile::common()
    };
    let mut with_meta = recording();
    with_meta[0].request.body["metadata"] = json!({ "user_id": "a" });
    let player = Player::new(with_meta, volatile);
    let mut sent = request_one();
    sent["metadata"] = json!({ "user_id": "b" });
    player.post_json(URL, &headers("k"), &sent).await.unwrap();
}

#[test]
fn replay_first_difference_points_at_the_first_divergence() {
    let a = json!({ "a": [1, { "b": "x" }], "c": 1 });
    assert_eq!(first_difference(&a, &a, ""), None);
    let b = json!({ "a": [1, { "b": "y" }], "c": 1 });
    assert_eq!(
        first_difference(&a, &b, "").unwrap(),
        "/a/1/b: recorded \"x\", sent \"y\""
    );
    let c = json!({ "a": [1, { "b": "x" }] });
    assert_eq!(
        first_difference(&a, &c, "").unwrap(),
        "/c: recorded has it, sent lacks it"
    );
}

/// A transport answering every request the same way and remembering what it saw.
struct Fake {
    seen: Mutex<Vec<Value>>,
    response: Response,
}

impl Transport for Fake {
    fn post_json<'a>(
        &'a self,
        _url: &'a str,
        _headers: &'a [(String, String)],
        body: &'a Value,
    ) -> TransportFuture<'a> {
        Box::pin(async move {
            self.seen.lock().unwrap().push(body.clone());
            Ok(self.response.clone())
        })
    }
}

#[tokio::test]
async fn recorder_writes_exchanges_with_secrets_stripped_that_the_player_replays() {
    let dir = tmp("record");
    let _ = std::fs::remove_dir_all(&dir);
    let key = "sk-ant-api03-SYNTHETIC-7781";
    let fake = Arc::new(Fake {
        seen: Mutex::new(Vec::new()),
        response: Response {
            status: 200,
            headers: [
                ("request-id".to_string(), "req_9".to_string()),
                ("set-cookie".to_string(), "sid=1".to_string()),
                ("anthropic-organization-id".to_string(), "org-SYNTHETIC-ORG".to_string()),
                ("anthropic-ratelimit-tokens-remaining".to_string(), "99".to_string()),
            ]
            .into_iter()
            .collect(),
            body: json!({ "id": "msg_9", "content": [{ "type": "text", "text": format!("echo {key}") }], "stop_reason": "end_turn", "usage": { "input_tokens": 1, "output_tokens": 1 } }).to_string(),
        },
    });
    let recorder = Recorder::new(
        fake.clone(),
        &dir,
        vec![key.to_string()],
        Volatile::common(),
    )
    .unwrap();
    let mut body = request_one();
    body["messages"][0]["content"][0]["text"] = json!(format!("my key is {key}"));
    let r = recorder.post_json(URL, &headers(key), &body).await.unwrap();
    assert_eq!(r.status, 200);
    let text = std::fs::read_to_string(dir.join("01.json")).unwrap();
    assert!(!text.contains("SYNTHETIC"), "{text}");
    assert!(
        text.contains(REDACTED) && !text.contains("set-cookie"),
        "{text}"
    );
    // Only the response headers the adapters read are kept; organisation ids and rate-limit
    // state never reach the recording.
    assert!(
        !text.contains("SYNTHETIC-ORG") && !text.contains("ratelimit"),
        "{text}"
    );
    let recorded = load(&dir).unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].request.headers["x-api-key"], REDACTED);
    assert_eq!(recorded[0].response.headers["request-id"], "req_9");
    assert_eq!(
        recorded[0].response.headers.keys().collect::<Vec<_>>(),
        ["request-id"]
    );
    // The recording replays against the same request (the key differs, which is volatile).
    let player = Player::load(&dir, Volatile::common()).unwrap();
    let mut scrubbed = body.clone();
    scrubbed["messages"][0]["content"][0]["text"] = json!(format!("my key is {REDACTED}"));
    player
        .post_json(URL, &headers("other"), &scrubbed)
        .await
        .unwrap();
    assert_eq!(fake.seen.lock().unwrap().len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.join("01.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
