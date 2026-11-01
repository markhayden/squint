//! Task 4.4a: the full client against the fake server: paginated listing with its guards,
//! concurrent calls on one process when the manifests allow and turns when they do not, the
//! frames in the context, exact content blocks, `outputSchema` validation, the `isError` truth
//! table in full, and a lease that waits for the previous run's calls.
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;
use squint_core::tool::{Manifest, Status};
use squint_testkit::mcp_server;

use super::*;

fn envelopes() -> String {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/tool-scripts/regions-3-then-1.json")
        .display()
        .to_string()
}

fn spawn(args: &[&str]) -> Spawn {
    let mut all: Vec<String> = vec!["--envelopes".into(), envelopes()];
    all.extend(args.iter().map(|s| s.to_string()));
    Spawn {
        command: mcp_server::path(),
        args: all,
        declared_env: vec![],
        cwd: None,
        extra_env: vec![],
    }
}

fn timeouts() -> Timeouts {
    Timeouts {
        startup: Duration::from_secs(10),
        call: Duration::from_secs(5),
        shutdown_grace: Duration::from_millis(300),
    }
}

fn env() -> Vec<(String, String)> {
    std::env::vars().collect()
}

fn context(run: &str) -> serde_json::Value {
    json!({ "run_id": run, "call_id": "call:1", "images": {}, "frames": {}, "hints": {} })
}

fn probe(pack: &Pack, run: &str) -> Envelope {
    pack.call(run, None, "probe_state", json!({ "_squint": context(run) }))
        .unwrap()
}

/// Waits until the server has written `line` on stderr (the fake server's `--trace` and pong
/// markers): an observed event, never a guess at how long one takes. Ten seconds is a hang.
fn wait_for_stderr(pack: &Pack, line: &str) {
    let until = Instant::now() + Duration::from_secs(10);
    while !pack.stderr_lines().iter().any(|l| l == line) {
        assert!(
            Instant::now() < until,
            "the server never wrote {line:?}: {:?}",
            pack.stderr_lines()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn measurement(e: &Envelope, name: &str) -> u64 {
    e.measurements
        .iter()
        .find(|m| m.name == name)
        .map(|m| m.value as u64)
        .unwrap()
}

#[test]
fn a_paginated_listing_is_followed_to_the_end() {
    let pack = Pack::connect("fake", spawn(&["--page-size", "1"]), timeouts(), env()).unwrap();
    let names: Vec<&str> = pack.manifests().iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["fake_regions", "probe_state"]);
}

#[test]
fn a_listing_that_never_ends_is_refused_as_resolve() {
    let err = Pack::connect("fake", spawn(&["--loop-cursor"]), timeouts(), env())
        .err()
        .unwrap();
    assert_eq!(err.class(), "resolve", "{err}");
    assert!(err.to_string().contains("never ends"), "{err}");
}

#[test]
fn concurrent_tools_run_together_on_one_process_and_serial_ones_take_turns() {
    // Two scripted calls at once: with `concurrent` manifests both are in flight together. The
    // server's own count proves it, not the clock: each call waits at the rendezvous until the
    // other arrives, so concurrent calls overlap however late a thread starts. A client that
    // serialised them would leave the first waiting out the rendezvous and report one in flight.
    let pack = Pack::connect(
        "fake",
        spawn(&["--concurrent", "--rendezvous-ms", "10000"]),
        timeouts(),
        env(),
    )
    .unwrap();
    assert!(pack.manifests().iter().all(|m| m.concurrent));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let pack = Arc::clone(&pack);
            std::thread::spawn(move || {
                pack.call(
                    "run-1",
                    None,
                    "fake_regions",
                    json!({ "image": "candidate", "_squint": context("run-1") }),
                )
            })
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap().unwrap().status, Status::Ok);
    }
    assert_eq!(measurement(&probe(&pack, "run-1"), "max_in_flight"), 2);

    // Without the declaration the same two calls take turns on the one process. The server
    // serves them on threads and holds each at the rendezvous for a while, so a client that sent
    // the second before the first was answered would be counted as two in flight; a call leaves
    // the count before its answer is written, so turns taken properly never overlap.
    let pack = Pack::connect(
        "fake",
        spawn(&["--threaded", "--rendezvous-ms", "300"]),
        timeouts(),
        env(),
    )
    .unwrap();
    assert!(pack.manifests().iter().all(|m| !m.concurrent));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let pack = Arc::clone(&pack);
            std::thread::spawn(move || {
                pack.call(
                    "run-1",
                    None,
                    "fake_regions",
                    json!({ "image": "candidate", "_squint": context("run-1") }),
                )
            })
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap().unwrap().status, Status::Ok);
    }
    assert_eq!(measurement(&probe(&pack, "run-1"), "max_in_flight"), 1);
}

#[test]
fn a_new_run_waits_for_the_previous_runs_calls_before_taking_the_lease() {
    let pack = Pack::connect(
        "fake",
        spawn(&["--concurrent", "--slow-call-ms", "500", "--trace"]),
        timeouts(),
        env(),
    )
    .unwrap();
    let slow = {
        let pack = Arc::clone(&pack);
        std::thread::spawn(move || {
            pack.call(
                "run-1",
                None,
                "fake_regions",
                json!({ "image": "candidate", "_squint": context("run-1") }),
            )
        })
    };
    // The server has the call, so run-1 holds the lease with a call in flight.
    wait_for_stderr(&pack, "fake mcp: start fake_regions");
    // run-2 arrives while run-1's call is in flight: it waits, then the server is reset for it.
    // The server's trace shows the order: run-1's call ended before the reset and run-2's probe.
    let e = probe(&pack, "run-2");
    wait_for_stderr(&pack, "fake mcp: end probe_state");
    let trace: Vec<String> = pack
        .stderr_lines()
        .into_iter()
        .filter(|l| {
            l.starts_with("fake mcp: start")
                || l.starts_with("fake mcp: end")
                || l == "fake mcp: reset"
        })
        .collect();
    assert_eq!(
        trace,
        [
            "fake mcp: start fake_regions",
            "fake mcp: end fake_regions",
            "fake mcp: reset",
            "fake mcp: start probe_state",
            "fake mcp: end probe_state",
        ],
        "waited for run-1's call"
    );
    assert_eq!(
        measurement(&e, "calls_since_reset"),
        1,
        "a fresh count after the reset"
    );
    assert_eq!(
        slow.join().unwrap().unwrap().status,
        Status::Ok,
        "run-1's call completed first"
    );
    assert_eq!(pack.restarts(), 0, "reset, not restart");
    // And a holder that never finishes within the deadline is given up on.
    let mut t = timeouts();
    t.startup = Duration::from_millis(300);
    let pack = Pack::connect(
        "fake",
        spawn(&["--concurrent", "--slow-call-ms", "1500", "--trace"]),
        t,
        env(),
    )
    .unwrap();
    let slow = {
        let pack = Arc::clone(&pack);
        std::thread::spawn(move || {
            pack.call(
                "run-1",
                None,
                "fake_regions",
                json!({ "image": "candidate", "_squint": context("run-1") }),
            )
        })
    };
    wait_for_stderr(&pack, "fake mcp: start fake_regions");
    let err = pack
        .call(
            "run-2",
            None,
            "probe_state",
            json!({ "_squint": context("run-2") }),
        )
        .err()
        .unwrap();
    assert!(err.to_string().contains("deadline"), "{err}");
    let _ = slow.join();
}

#[test]
fn a_result_outside_its_output_schema_or_with_a_stray_block_is_a_tool_error() {
    let pack = Pack::connect("fake", spawn(&["--extra-measurement"]), timeouts(), env()).unwrap();
    let err = pack
        .call(
            "run-1",
            None,
            "probe_state",
            json!({ "_squint": context("run-1") }),
        )
        .err()
        .unwrap();
    assert_eq!(err.class(), "tool");
    assert!(err.to_string().contains("outputSchema"), "{err}");
    let pack = Pack::connect("fake", spawn(&["--text-block"]), timeouts(), env()).unwrap();
    let err = pack
        .call(
            "run-1",
            None,
            "probe_state",
            json!({ "_squint": context("run-1") }),
        )
        .err()
        .unwrap();
    assert_eq!(err.class(), "tool");
    assert!(err.to_string().contains("\"text\" block"), "{err}");
}

fn envelope_json(status: &str) -> serde_json::Value {
    json!({
        "status": status,
        "summary": "s",
        "measurements": [],
        "regions": [],
        "images": [],
        "transforms": [],
        "provenance": { "tool": "t", "version": "1", "duration_ms": 1 },
    })
}

fn manifest() -> Manifest {
    Manifest::from_toml(squint_testkit::tool::FAKE_REGIONS_MANIFEST).unwrap()
}

/// SPEC "Results": every combination of `isError` and envelope status, the JSON-RPC error, and
/// a block mismatch. The one recoverable form is `isError` with `invalid_args`.
#[test]
fn the_is_error_truth_table_is_exhaustive() {
    let m = manifest();
    for status in ["ok", "measured_nothing", "not_applicable", "invalid_args"] {
        for is_error in [false, true] {
            let result = CallResult {
                structured: envelope_json(status),
                content: vec![],
                is_error,
            };
            let outcome = envelope_from(result, &m);
            match (is_error, status) {
                (false, _) => assert!(outcome.is_ok(), "isError false, {status}: accepted as is"),
                (true, "invalid_args") => {
                    assert_eq!(
                        outcome.unwrap().status,
                        Status::InvalidArgs,
                        "the recoverable form"
                    )
                }
                (true, _) => {
                    let err = outcome.err().unwrap();
                    assert_eq!(err.class(), "tool", "isError true, {status}");
                    assert!(err.to_string().contains("isError"), "{err}");
                }
            }
        }
    }
    // A JSON-RPC error never reaches envelope_from: `request` turns it into error: tool.
    // Block mismatches: a block for no image, an image with no block, a wrong media type, and an
    // image whose block carries its bytes.
    let one_image = |media_type: &str| {
        let mut e = envelope_json("ok");
        e["images"] = json!([{ "frame": "crop:call:1:0", "parent": "raw:candidate", "mapping": { "bbox": [0.0, 0.0, 0.5, 0.5] }, "media_type": media_type, "width": 2, "height": 2 }]);
        e
    };
    let block = |mime: &str| json!({ "type": "image", "data": "AAAA", "mimeType": mime });
    assert!(
        envelope_from(
            CallResult {
                structured: envelope_json("ok"),
                content: vec![block("image/png")],
                is_error: false
            },
            &m
        )
        .is_err()
    );
    assert!(
        envelope_from(
            CallResult {
                structured: one_image("image/png"),
                content: vec![],
                is_error: false
            },
            &m
        )
        .is_err()
    );
    assert!(
        envelope_from(
            CallResult {
                structured: one_image("image/png"),
                content: vec![block("image/jpeg")],
                is_error: false
            },
            &m
        )
        .is_err()
    );
    let e = envelope_from(
        CallResult {
            structured: one_image("image/png"),
            content: vec![block("image/png")],
            is_error: false,
        },
        &m,
    )
    .unwrap();
    assert_eq!(e.images[0].base64.as_deref(), Some("AAAA"));
}

/// A conforming server may ping; the client answers and no call in flight is disturbed.
#[test]
fn a_servers_ping_is_answered_and_disturbs_no_call() {
    let pack = Pack::connect("fake", spawn(&["--ping"]), timeouts(), env()).unwrap();
    // The server says when the pong arrived; the probe then reports it.
    wait_for_stderr(&pack, "fake mcp: pong");
    let e = probe(&pack, "run-1");
    assert_eq!(e.status, Status::Ok);
    assert!(e.summary.contains("pongs=1"), "{}", e.summary);
    assert_eq!(pack.restarts(), 0);
}

/// A failed call carries the server's recent stderr, the one diagnostic left after it died.
#[test]
fn a_failed_call_carries_the_servers_stderr() {
    let pack = Pack::connect("fake", spawn(&["--die-on-call"]), timeouts(), env()).unwrap();
    let err = pack
        .call(
            "run-1",
            None,
            "probe_state",
            json!({ "_squint": context("run-1") }),
        )
        .err()
        .unwrap();
    assert!(
        err.to_string()
            .contains("server stderr: fake mcp: dying on call"),
        "{err}"
    );
}

/// A dying server's stderr can end after its stdout: the error still carries the last lines,
/// because it waits for the end of stderr rather than taking what has arrived so far.
#[test]
fn stderr_written_after_stdout_ends_still_reaches_the_error() {
    let client = Client::connect(
        &spawn(&["--die-on-call", "--late-stderr-ms", "300"]),
        env(),
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    let err = client
        .call(
            "probe_state",
            json!({ "_squint": context("run-1") }),
            Duration::from_secs(5),
        )
        .err()
        .unwrap();
    let text = err.to_string();
    assert!(text.contains("ended the conversation"), "{text}");
    assert!(
        text.contains("server stderr: fake mcp: last words"),
        "{text}"
    );
    client.shutdown(Duration::from_millis(300));
}

/// A server whose child holds stderr open long after the server died does not hang the error:
/// it waits at most `STDERR_DRAIN`, then reports what arrived.
#[test]
fn a_stderr_held_open_by_a_child_delays_the_error_by_a_bounded_wait() {
    let client = Client::connect(
        &spawn(&["--die-on-call", "--late-stderr-ms", "5000"]),
        env(),
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    let started = Instant::now();
    let err = client
        .call(
            "probe_state",
            json!({ "_squint": context("run-1") }),
            Duration::from_secs(30),
        )
        .err()
        .unwrap();
    let waited = started.elapsed();
    assert!(err.to_string().contains("ended the conversation"), "{err}");
    assert!(!err.to_string().contains("last words"), "{err}");
    assert!(
        waited >= STDERR_DRAIN && waited < STDERR_DRAIN + Duration::from_secs(3),
        "waited {waited:?}"
    );
    client.shutdown(Duration::from_millis(300));
}

/// A stderr line that is not UTF-8 stops nothing: it arrives with its bad bytes replaced, and
/// the lines after it still reach the error of a call the server died on.
#[test]
fn stderr_that_is_not_utf8_loses_no_later_line() {
    let client = Client::connect(
        &spawn(&["--bad-stderr", "--die-on-call"]),
        env(),
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    let err = client
        .call(
            "probe_state",
            json!({ "_squint": context("run-1") }),
            Duration::from_secs(5),
        )
        .err()
        .unwrap();
    let text = err.to_string();
    assert!(
        text.contains("fake mcp: \u{FFFD}\u{FFFD} bad bytes | fake mcp: after the bad bytes"),
        "{text}"
    );
    assert!(text.contains("fake mcp: dying on call"), "{text}");
    client.shutdown(Duration::from_millis(300));
}

/// More non-UTF-8 stderr than a pipe holds, written before each answer: the reader drains all of
/// it, so the calls complete and the newest lines are kept, the long one cut at the bound.
#[test]
fn a_flood_of_non_utf8_stderr_never_blocks_the_server() {
    let client = Client::connect(
        &spawn(&["--stderr-flood-kib", "256"]),
        env(),
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    for _ in 0..2 {
        let e = client
            .call(
                "probe_state",
                json!({ "_squint": context("run-1") }),
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(e.structured["status"], "ok");
    }
    // The answer can overtake the stderr written before it (another pipe, another thread), so
    // the lines are read once the process is gone and the reader has reached the end.
    client.shutdown(Duration::from_millis(300));
    client.shared.drain_stderr(Duration::from_secs(10));
    let lines = client.stderr_lines();
    assert_eq!(lines.len(), STDERR_LINES);
    let long = lines.last().unwrap();
    assert!(long.ends_with(TRUNCATED), "the long line is cut");
    assert_eq!(
        long.chars().filter(|c| *c == '\u{FFFD}').count(),
        STDERR_LINE_BYTES
    );
}

/// The line reader on its own: CRLF and LF endings, an unterminated last line, empty lines,
/// invalid bytes and a line past the bound, which is cut and then consumed to its end.
#[test]
fn stderr_lines_are_split_decoded_and_bounded() {
    let mut input = b"one\r\n\ntwo \xC3(\n".to_vec();
    input.extend(std::iter::repeat_n(b'x', STDERR_LINE_BYTES + 10));
    input.extend(b"\nafter\nlast");
    let mut lines = Vec::new();
    // A one-byte buffer makes every line span many reads.
    read_stderr_lines(std::io::BufReader::with_capacity(1, &input[..]), |l| {
        lines.push(l)
    });
    let long = format!("{}{TRUNCATED}", "x".repeat(STDERR_LINE_BYTES));
    assert_eq!(
        lines,
        ["one", "", "two \u{FFFD}(", long.as_str(), "after", "last"]
    );
}

// ---- task 4.4b: failure modes ----------------------------------------------------------------

/// A server that dies mid-call fails the call as `error: tool` and is replaced for the next one.
#[test]
fn a_server_that_dies_mid_call_fails_the_call_and_is_replaced() {
    let pack = Pack::connect("fake", spawn(&["--die-on-call"]), timeouts(), env()).unwrap();
    let pid_a = pack.pid().unwrap();
    let err = pack
        .call(
            "run-1",
            None,
            "probe_state",
            json!({ "_squint": context("run-1") }),
        )
        .err()
        .unwrap();
    assert_eq!(err.class(), "tool");
    assert!(err.to_string().contains("ended the conversation"), "{err}");
    // The replacement dies the same way, but it is a fresh process each time.
    let _ = pack.call(
        "run-1",
        None,
        "probe_state",
        json!({ "_squint": context("run-1") }),
    );
    assert_ne!(pack.pid(), Some(pid_a));
    assert_eq!(pack.restarts(), 1);
}

/// A malformed frame ends the conversation: the pending call fails and the process is replaced.
#[test]
fn a_malformed_frame_fails_the_call_and_taints_the_process() {
    let pack = Pack::connect("fake", spawn(&["--garbage-line"]), timeouts(), env()).unwrap();
    let pid_a = pack.pid().unwrap();
    let err = pack
        .call(
            "run-1",
            None,
            "probe_state",
            json!({ "_squint": context("run-1") }),
        )
        .err()
        .unwrap();
    assert_eq!(err.class(), "tool");
    assert!(err.to_string().contains("malformed"), "{err}");
    let _ = pack.call(
        "run-1",
        None,
        "probe_state",
        json!({ "_squint": context("run-1") }),
    );
    assert_ne!(pack.pid(), Some(pid_a), "never reused");
}

/// A response for an id nobody asked about fails the pending call and taints the process.
#[test]
fn a_response_for_an_unknown_id_fails_the_call_and_taints_the_process() {
    let pack = Pack::connect("fake", spawn(&["--stray-response"]), timeouts(), env()).unwrap();
    let pid_a = pack.pid().unwrap();
    let err = pack
        .call(
            "run-1",
            None,
            "probe_state",
            json!({ "_squint": context("run-1") }),
        )
        .err()
        .unwrap();
    assert_eq!(err.class(), "tool");
    assert!(err.to_string().contains("unknown id 999999"), "{err}");
    let _ = pack.call(
        "run-1",
        None,
        "probe_state",
        json!({ "_squint": context("run-1") }),
    );
    assert_ne!(pack.pid(), Some(pid_a));
}

/// A late answer to a cancelled call is dropped by the reader, and the tainted process is never
/// asked anything again: the next call runs on a fresh one even within the same run.
#[test]
fn a_late_answer_after_cancellation_is_ignored() {
    let mut t = timeouts();
    t.call = Duration::from_millis(200);
    let client = Client::connect(
        &spawn(&["--slow-call-ms", "600"]),
        env(),
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    let err = client
        .call(
            "fake_regions",
            json!({ "image": "candidate", "_squint": context("run-1") }),
            t.call,
        )
        .err()
        .unwrap();
    assert!(err.to_string().contains("timed out"), "{err}");
    assert!(client.is_tainted());
    // The late answer arrives while nothing waits; it is swallowed, and a later request on this
    // process still gets its own answer (the pack would have replaced the process by now).
    std::thread::sleep(Duration::from_millis(700));
    let e = client
        .call(
            "probe_state",
            json!({ "_squint": context("run-1") }),
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(e.structured["status"], "ok");
    assert!(
        client
            .stderr_lines()
            .iter()
            .any(|l| l.contains("cancelled requestId")),
        "the server saw the cancellation: {:?}",
        client.stderr_lines()
    );
    client.shutdown(Duration::from_millis(300));
}

/// Two decisions share one pack: their runs are told apart by the run directory, so a `run-1`
/// of a second decision takes a lease of its own rather than inheriting the first's.
#[test]
fn runs_of_different_decisions_hold_different_leases_on_a_shared_pack() {
    let pack = Pack::connect("fake", spawn(&[]), timeouts(), env()).unwrap();
    let a = std::env::temp_dir().join("squint-lease-a");
    let b = std::env::temp_dir().join("squint-lease-b");
    let probe_in = |dir: &std::path::Path| {
        pack.call(
            "run-1",
            Some(dir),
            "probe_state",
            json!({ "_squint": context("run-1") }),
        )
        .unwrap()
    };
    assert_eq!(measurement(&probe_in(&a), "calls_since_reset"), 1);
    assert_eq!(
        measurement(&probe_in(&a), "calls_since_reset"),
        2,
        "the same run keeps its lease"
    );
    assert_eq!(
        measurement(&probe_in(&b), "calls_since_reset"),
        1,
        "another decision's run-1 starts afresh"
    );
    assert_eq!(pack.restarts(), 0, "reset, not restart");
}
