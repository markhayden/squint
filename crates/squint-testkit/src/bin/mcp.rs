//! The fake MCP server (stdio, revision 2025-11-25) the tests drive: it serves `fake_regions` with
//! scripted envelopes and `probe_state`, which reports the calls since start or reset, its pid,
//! and which test variables reached its environment. Flags make it misbehave on purpose.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

struct Options {
    protocol_version: String,
    tools_capability: bool,
    reset: bool,
    hang_reset: bool,
    slow_call: Duration,
    ignore_stdin_close: bool,
    envelopes: VecDeque<Value>,
    /// Tools per `tools/list` page (0: all in one page).
    page_size: usize,
    /// Every page names the same next cursor: a listing that never ends.
    loop_cursor: bool,
    /// Manifests declare `concurrent = true` and calls are served on threads.
    concurrent: bool,
    /// `probe_state` reports a measurement its output schema does not allow.
    extra_measurement: bool,
    /// Every result carries a stray text content block.
    text_block: bool,
    /// The process exits as soon as a `tools/call` arrives.
    die_on_call: bool,
    /// With `die_on_call`, the dying words come late: the process exits at once, leaving a child
    /// that holds its stderr open, writes `fake mcp: last words` after this many milliseconds and
    /// exits. Stdout ends well before stderr, as it can when a real server dies.
    late_stderr_ms: Option<u64>,
    /// Calls are served on threads although the manifests do not declare `concurrent`, so a
    /// client that broke the turns would show as `max_in_flight` above one.
    threaded: bool,
    /// `fake_regions` waits until another call has been in flight with it (`max_in_flight` two),
    /// or this many milliseconds pass:
    /// calls the client really sends together always overlap, however late a thread starts.
    rendezvous: Duration,
    /// Each call writes `fake mcp: start <tool>` on stderr as it begins and `fake mcp: end <tool>`
    /// just before its answer, and an acknowledged reset `fake mcp: reset`, so a test can wait
    /// for a call to be in flight and check the order of events.
    trace: bool,
    /// At start, a stderr line that is not UTF-8, then `fake mcp: after the bad bytes`.
    bad_stderr: bool,
    /// Before answering each call, this many KiB of non-UTF-8 on stderr: short lines, then one
    /// line far longer than any reader keeps. More than a pipe holds, so every byte must be
    /// drained while the server writes.
    stderr_flood_kib: usize,
    /// The child of `late_stderr_ms`: wait this many milliseconds, write the last words, exit.
    say_after_ms: Option<u64>,
    /// The process exits after answering this many calls (0: never): a server that dies between
    /// runs or between the calls of one run.
    exit_after_calls: u64,
    /// A line that is not JSON precedes every call's response.
    garbage_line: bool,
    /// A response for an id nobody asked about precedes every call's response.
    stray_response: bool,
    /// The server pings the client after `initialize`; `probe_state` reports whether a pong came.
    ping: bool,
}

fn options() -> Options {
    let mut o = Options {
        protocol_version: "2025-11-25".into(),
        tools_capability: true,
        reset: true,
        hang_reset: false,
        slow_call: Duration::ZERO,
        ignore_stdin_close: false,
        envelopes: VecDeque::new(),
        page_size: 0,
        loop_cursor: false,
        concurrent: false,
        extra_measurement: false,
        text_block: false,
        die_on_call: false,
        late_stderr_ms: None,
        say_after_ms: None,
        threaded: false,
        rendezvous: Duration::ZERO,
        trace: false,
        bad_stderr: false,
        stderr_flood_kib: 0,
        exit_after_calls: 0,
        garbage_line: false,
        stray_response: false,
        ping: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--protocol-version" => o.protocol_version = args.next().unwrap_or_default(),
            "--no-tools-capability" => o.tools_capability = false,
            "--no-reset" => o.reset = false,
            "--hang-reset" => o.hang_reset = true,
            "--slow-call-ms" => {
                o.slow_call =
                    Duration::from_millis(args.next().and_then(|s| s.parse().ok()).unwrap_or(0));
            }
            "--ignore-stdin-close" => o.ignore_stdin_close = true,
            "--page-size" => o.page_size = args.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--loop-cursor" => o.loop_cursor = true,
            "--concurrent" => o.concurrent = true,
            "--extra-measurement" => o.extra_measurement = true,
            "--text-block" => o.text_block = true,
            "--die-on-call" => o.die_on_call = true,
            "--late-stderr-ms" => o.late_stderr_ms = args.next().and_then(|s| s.parse().ok()),
            "--threaded" => o.threaded = true,
            "--rendezvous-ms" => {
                o.rendezvous =
                    Duration::from_millis(args.next().and_then(|s| s.parse().ok()).unwrap_or(0));
            }
            "--trace" => o.trace = true,
            "--bad-stderr" => o.bad_stderr = true,
            "--stderr-flood-kib" => {
                o.stderr_flood_kib = args.next().and_then(|s| s.parse().ok()).unwrap_or(0)
            }
            "--say-after-ms" => o.say_after_ms = args.next().and_then(|s| s.parse().ok()),
            "--exit-after-calls" => {
                o.exit_after_calls = args.next().and_then(|s| s.parse().ok()).unwrap_or(0)
            }
            "--garbage-line" => o.garbage_line = true,
            "--stray-response" => o.stray_response = true,
            "--ping" => o.ping = true,
            "--envelopes" => {
                let path = args.next().unwrap_or_default();
                let text = std::fs::read_to_string(&path).unwrap_or_else(|_| "[]".into());
                let list: Vec<Value> = serde_json::from_str(&text).unwrap_or_default();
                o.envelopes = list.into();
            }
            other => eprintln!("fake mcp: unknown flag {other}"),
        }
    }
    o
}

fn manifest(toml_text: &str) -> Value {
    let parsed: toml::Value = toml::from_str(toml_text).expect("manifest toml");
    serde_json::to_value(&parsed["tool"]).expect("manifest json")
}

/// A squint manifest as an MCP tool: native name, description, schemas; the rest annotated.
fn mcp_tool(manifest: &Value) -> Value {
    let mut annotation = manifest.clone();
    if let Some(obj) = annotation.as_object_mut() {
        obj.remove("name");
        obj.remove("input_schema");
        obj.remove("output_schema");
    }
    json!({
        "name": manifest["name"],
        "description": manifest["purpose"],
        "inputSchema": manifest["input_schema"],
        "outputSchema": manifest["output_schema"],
        "annotations": { "squint": annotation },
    })
}

fn provenance(tool: &str) -> Value {
    json!({ "tool": tool, "version": "0.0.1", "duration_ms": 1, "lock_digest": null })
}

fn invalid_args(tool: &str, message: &str) -> Value {
    json!({
        "status": "invalid_args", "summary": message, "measurements": [], "regions": [], "images": [], "transforms": [],
        "provenance": provenance(tool),
    })
}

/// State the call handlers share, whether they run inline or on threads.
struct Server {
    options: Options,
    envelopes: Mutex<VecDeque<Value>>,
    stdout: Mutex<std::io::Stdout>,
    calls_since_reset: AtomicU64,
    calls_total: AtomicU64,
    in_flight: AtomicU64,
    max_in_flight: AtomicU64,
    /// Pongs received for the server's pings.
    pongs: AtomicU64,
}

impl Server {
    fn respond(&self, value: Value) {
        if let Ok(mut out) = self.stdout.lock() {
            let _ = writeln!(out, "{value}");
            let _ = out.flush();
        }
    }

    /// Writes a line that is not JSON.
    fn garbage(&self) {
        if let Ok(mut out) = self.stdout.lock() {
            let _ = writeln!(out, "this is not a frame");
            let _ = out.flush();
        }
    }

    /// `tools/call`: the scripted tool, the state probe, or an unknown tool.
    fn call(&self, id: Option<Value>, params: &Value) {
        let o = &self.options;
        if o.stderr_flood_kib > 0 {
            flood_stderr(o.stderr_flood_kib * 1024);
        }
        let in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(in_flight, Ordering::SeqCst);
        let calls = self.calls_since_reset.fetch_add(1, Ordering::SeqCst) + 1;
        let name = params["name"].as_str().unwrap_or("");
        let args = &params["arguments"];
        if o.trace {
            eprintln!("fake mcp: start {name}");
        }
        // Only the scripted tool is slow, so a probe can still observe the server.
        if name == "fake_regions" && !o.slow_call.is_zero() {
            std::thread::sleep(o.slow_call);
        }
        if name == "fake_regions" && !o.rendezvous.is_zero() {
            let until = std::time::Instant::now() + o.rendezvous;
            // The high-water mark, not the current count: a peer that met us and finished
            // between two looks still counts.
            while self.max_in_flight.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < until
            {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let (envelope, is_error) = if args.get("_squint").is_none() {
            (invalid_args(name, "_squint is required"), true)
        } else {
            match name {
                "fake_regions" => {
                    match self.envelopes.lock().ok().and_then(|mut e| e.pop_front()) {
                        Some(e) => (e, false),
                        None => (invalid_args(name, "script exhausted"), true),
                    }
                }
                "probe_state" => {
                    let mut measurements = vec![
                        json!({ "name": "calls_since_reset", "value": calls, "unit": "count", "frame": null }),
                        json!({ "name": "pid", "value": std::process::id(), "unit": "count", "frame": null }),
                        json!({ "name": "max_in_flight", "value": self.max_in_flight.load(Ordering::SeqCst), "unit": "count", "frame": null }),
                    ];
                    if o.extra_measurement {
                        measurements.push(
                            json!({ "name": "bogus", "value": 1, "unit": "count", "frame": null }),
                        );
                    }
                    (
                        json!({
                            "status": "ok",
                            "summary": format!(
                                "calls_since_reset={calls} plain={} secret={} pongs={}",
                                u8::from(std::env::var_os("SQUINT_TEST_PLAIN").is_some()),
                                u8::from(std::env::var_os("SQUINT_TEST_SECRET_KEY").is_some()),
                                self.pongs.load(Ordering::SeqCst)
                            ),
                            "measurements": measurements,
                            "regions": [], "images": [], "transforms": [],
                            "provenance": provenance(name),
                        }),
                        false,
                    )
                }
                _ => {
                    self.respond(
                        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32602, "message": format!("unknown tool {name}") } }),
                    );
                    self.in_flight.fetch_sub(1, Ordering::SeqCst);
                    return;
                }
            }
        };
        let mut content: Vec<Value> = envelope["images"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|img| json!({ "type": "image", "data": img["base64"].as_str().unwrap_or(""), "mimeType": img["media_type"] }))
            .collect();
        if o.text_block {
            content.push(json!({ "type": "text", "text": "a stray note" }));
        }
        // Out of flight before the answer leaves: a client that waits for it to send the next
        // call can never be counted as overlapping it.
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        if o.trace {
            eprintln!("fake mcp: end {name}");
        }
        self.respond(
            json!({ "jsonrpc": "2.0", "id": id, "result": { "content": content, "structuredContent": envelope, "isError": is_error } }),
        );
        let total = self.calls_total.fetch_add(1, Ordering::SeqCst) + 1;
        if o.exit_after_calls > 0 && total >= o.exit_after_calls {
            eprintln!("fake mcp: exiting after {total} calls");
            std::process::exit(0);
        }
    }
}

/// `total` bytes of 0xFF on stderr: half in lines of a hundred bytes, half in one line.
fn flood_stderr(total: usize) {
    let mut err = std::io::stderr().lock();
    let half = total / 2;
    let mut line = vec![0xFF_u8; 99];
    line.push(b'\n');
    for _ in 0..half / line.len() {
        let _ = err.write_all(&line);
    }
    let mut long = vec![0xFF_u8; total - half];
    long.push(b'\n');
    let _ = err.write_all(&long);
    let _ = err.flush();
}

fn main() {
    let mut o = options();
    if o.bad_stderr {
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(b"fake mcp: \xFF\xFE bad bytes\n");
        let _ = err.write_all(b"fake mcp: after the bad bytes\n");
        let _ = err.flush();
    }
    if let Some(ms) = o.say_after_ms {
        std::thread::sleep(Duration::from_millis(ms));
        eprintln!("fake mcp: last words");
        return;
    }
    let envelopes = std::mem::take(&mut o.envelopes);
    let mut fake_regions = manifest(include_str!("../../fixtures/fake_regions.toml"));
    let mut probe_state = manifest(include_str!("../../fixtures/probe_state.toml"));
    if o.concurrent {
        fake_regions["concurrent"] = json!(true);
        probe_state["concurrent"] = json!(true);
    }
    let listed = vec![mcp_tool(&fake_regions), mcp_tool(&probe_state)];
    let server = Arc::new(Server {
        options: o,
        envelopes: Mutex::new(envelopes),
        stdout: Mutex::new(std::io::stdout()),
        calls_since_reset: AtomicU64::new(0),
        calls_total: AtomicU64::new(0),
        in_flight: AtomicU64::new(0),
        max_in_flight: AtomicU64::new(0),
        pongs: AtomicU64::new(0),
    });
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            eprintln!("fake mcp: malformed line");
            continue;
        };
        let method = msg["method"].as_str().unwrap_or("");
        let id = msg.get("id").cloned();
        let o = &server.options;
        // The client's answer to our ping: a response (no method) for the ping's id.
        if method.is_empty() && id == Some(json!(900_001)) {
            server.pongs.fetch_add(1, Ordering::SeqCst);
            eprintln!("fake mcp: pong");
            continue;
        }
        match method {
            "initialize" => {
                let mut capabilities = json!({});
                if o.tools_capability {
                    capabilities["tools"] = json!({ "listChanged": false });
                }
                server.respond(json!({ "jsonrpc": "2.0", "id": id, "result": {
                    "protocolVersion": o.protocol_version,
                    "capabilities": capabilities,
                    "serverInfo": { "name": "squint-testkit-mcp", "version": "0.0.1" },
                } }));
            }
            "notifications/initialized" => {
                if o.ping {
                    server.respond(json!({ "jsonrpc": "2.0", "id": 900_001, "method": "ping" }));
                }
            }
            "notifications/cancelled" => {
                eprintln!(
                    "fake mcp: cancelled requestId {}",
                    msg["params"]["requestId"]
                );
            }
            "tools/list" => {
                // Pages of `page_size` tools, the cursor the index of the next one; with
                // `loop_cursor` the same cursor forever.
                let start: usize = msg["params"]["cursor"]
                    .as_str()
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(0);
                let (page, next): (Vec<Value>, Option<String>) = if o.loop_cursor {
                    (listed.clone(), Some("again".into()))
                } else if o.page_size == 0 {
                    (listed.clone(), None)
                } else {
                    let end = (start + o.page_size).min(listed.len());
                    let next = (end < listed.len()).then(|| end.to_string());
                    (listed[start.min(listed.len())..end].to_vec(), next)
                };
                let mut result = json!({ "tools": page });
                if let Some(n) = next {
                    result["nextCursor"] = json!(n);
                }
                server.respond(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
            }
            "tools/call" => {
                if o.die_on_call {
                    match o.late_stderr_ms {
                        Some(ms) => {
                            // The child inherits stderr only, so stdout closes when we exit.
                            let _ = std::env::current_exe().and_then(|exe| {
                                std::process::Command::new(exe)
                                    .args(["--say-after-ms", &ms.to_string()])
                                    .stdin(std::process::Stdio::null())
                                    .stdout(std::process::Stdio::null())
                                    .spawn()
                            });
                        }
                        None => eprintln!("fake mcp: dying on call"),
                    }
                    std::process::exit(3);
                }
                if o.garbage_line {
                    server.garbage();
                }
                if o.stray_response {
                    server.respond(json!({ "jsonrpc": "2.0", "id": 999_999, "result": {} }));
                }
                let params = msg["params"].clone();
                if o.concurrent || o.threaded {
                    let s = Arc::clone(&server);
                    std::thread::spawn(move || s.call(id, &params));
                } else {
                    server.call(id, &params);
                }
            }
            "squint/reset" => {
                if o.hang_reset {
                    eprintln!("fake mcp: hanging on reset");
                    continue;
                }
                if !o.reset {
                    server.respond(
                        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "Method not found" } }),
                    );
                    continue;
                }
                if o.trace {
                    eprintln!("fake mcp: reset");
                }
                server.calls_since_reset.store(0, Ordering::SeqCst);
                server.max_in_flight.store(0, Ordering::SeqCst);
                server.respond(json!({ "jsonrpc": "2.0", "id": id, "result": {} }));
            }
            _ => {
                if id.is_some() {
                    server.respond(
                        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "Method not found" } }),
                    );
                }
            }
        }
    }
    if server.options.ignore_stdin_close {
        eprintln!("fake mcp: ignoring stdin close");
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}
