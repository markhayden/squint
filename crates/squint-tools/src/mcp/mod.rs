//! The MCP client (SPEC "Transports", MCP over stdio, revision 2025-11-25; ADR 0015 keeps it
//! in-house). The client is synchronous, like the `Tool` trait it serves, and multiplexed: a
//! reader thread routes each response to the request waiting for its id, so tools whose manifests
//! declare `concurrent = true` may have several calls in flight on one process while every wait
//! keeps its own deadline. A call that passes its deadline is cancelled with
//! `notifications/cancelled`, its id tombstoned so a late answer is ignored, and the process marked
//! unreusable. A response for an unknown id, a malformed frame or the end of the stream fails
//! every pending request and taints the process. When the stream ends because the server exited,
//! the error waits (at most `STDERR_DRAIN`) for the server's stderr to close, so a dying server's
//! last lines are in it.

pub mod pack;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use squint_core::tool::{Envelope, Manifest, Status};
use squint_core::{Error, Result};

pub use pack::{Pack, PackTool};

/// The one revision the first release speaks.
pub const PROTOCOL_VERSION: &str = "2025-11-25";

/// `tools/list` pages followed before the listing is refused as unbounded.
pub const MAX_LIST_PAGES: usize = 64;

/// How to start a server.
#[derive(Debug, Clone)]
pub struct Spawn {
    pub command: PathBuf,
    pub args: Vec<String>,
    /// Environment variable names the pack declares; filtered by `squint_core::env::allowlist`.
    pub declared_env: Vec<String>,
    /// The working directory (the run directory); the current directory when `None`.
    pub cwd: Option<PathBuf>,
    /// Harness-owned variables set after the allowlist (`UV_OFFLINE`, the lock digest).
    pub extra_env: Vec<(String, String)>,
}

/// The clocks of the contract.
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    /// One bounded deadline for acquiring a lease: startup, reset or restart.
    pub startup: Duration,
    /// Per-call deadline (`budget.tool_timeout_s`).
    pub call: Duration,
    /// Each step of shutdown escalation waits this long: stdin closed, SIGTERM, SIGKILL.
    pub shutdown_grace: Duration,
}

/// A tool as the server listed it, with its manifest assembled from the native fields and the
/// `squint` annotation.
#[derive(Debug, Clone)]
pub struct ListedTool {
    pub name: String,
    pub manifest: Manifest,
}

/// What `tools/call` returned.
#[derive(Debug, Clone)]
pub struct CallResult {
    pub structured: Value,
    pub content: Vec<Value>,
    pub is_error: bool,
}

/// Whether the server honoured `squint/reset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reset {
    Acknowledged,
    Unsupported,
}

/// What a waiting request receives: the server's message for its id, or why it never will.
type Delivery = std::result::Result<Value, String>;

/// State the reader thread and every request share.
struct Shared {
    pending: Mutex<HashMap<u64, Sender<Delivery>>>,
    tombstones: Mutex<BTreeSet<u64>>,
    /// A call timed out or was cancelled, or the stream broke: never reuse this process.
    tainted: AtomicBool,
    /// The server's last `STDERR_LINES` lines, for the error a failed call carries.
    stderr: Mutex<Vec<String>>,
    /// Set when the stderr reader has reached the end of the pipe.
    stderr_closed: Mutex<bool>,
    stderr_closed_signal: Condvar,
    /// Set by the stdout reader when the stream ends, before it fails the pending requests.
    stdout_closed: AtomicBool,
    stdin: Mutex<Option<ChildStdin>>,
}

/// Lines of the server's stderr kept.
const STDERR_LINES: usize = 200;

/// Bytes of one stderr line kept; the rest of a longer line is read and dropped.
const STDERR_LINE_BYTES: usize = 4096;

/// Marks a stderr line cut at `STDERR_LINE_BYTES`.
const TRUNCATED: &str = " [truncated]";

/// Reads the server's stderr to the end of the pipe whatever it holds, handing `each` every line
/// (without its `\n` or `\r\n`) decoded lossily and cut at `STDERR_LINE_BYTES`. It never stops
/// early: a reader that quit on a bad byte would lose every later line and close the pipe under
/// a server still writing to it, whose next write then fails with a broken pipe (fatal to many
/// servers); one that stopped reading but kept the pipe open would leave the server blocked on a
/// full pipe. Only a read error ends it before the end of the pipe.
fn read_stderr_lines(mut reader: impl BufRead, mut each: impl FnMut(String)) {
    let mut line: Vec<u8> = Vec::new();
    let mut truncated = false;
    let mut emit = |line: &mut Vec<u8>, truncated: &mut bool| {
        if line.last() == Some(&b'\r') && !*truncated {
            line.pop();
        }
        let mut text = String::from_utf8_lossy(line).into_owned();
        if *truncated {
            text.push_str(TRUNCATED);
        }
        each(text);
        line.clear();
        *truncated = false;
    };
    loop {
        let buf = match reader.fill_buf() {
            Ok([]) => break,
            Ok(buf) => buf,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let (chunk, used, ends_line) = match buf.iter().position(|&b| b == b'\n') {
            Some(i) => (&buf[..i], i + 1, true),
            None => (buf, buf.len(), false),
        };
        let room = STDERR_LINE_BYTES.saturating_sub(line.len());
        if chunk.len() > room {
            truncated = true;
        }
        line.extend_from_slice(&chunk[..chunk.len().min(room)]);
        reader.consume(used);
        if ends_line {
            emit(&mut line, &mut truncated);
        }
    }
    if !line.is_empty() || truncated {
        emit(&mut line, &mut truncated);
    }
}

/// How long an error about a server that ended the conversation waits for its stderr to close.
/// The stdout and stderr pipes are read by different threads, so the end of stdout can arrive
/// before the last lines a dying server wrote to stderr; those lines are often the only
/// diagnostic left, so the error waits for the end of stderr (the pipe closes when the process
/// exits). The bound covers a server that closed stdout but lives on, or whose child still holds
/// stderr open: the error then carries whatever arrived in time.
const STDERR_DRAIN: Duration = Duration::from_secs(1);

impl Shared {
    fn write(&self, message: &Value) -> Result<()> {
        let mut guard = self
            .stdin
            .lock()
            .map_err(|_| tool_err("the server's stdin lock is poisoned"))?;
        let stdin = guard
            .as_mut()
            .ok_or_else(|| tool_err("the server's stdin is closed"))?;
        let mut text = serde_json::to_string(message)?;
        text.push('\n');
        stdin
            .write_all(text.as_bytes())
            .and_then(|()| stdin.flush())
            .map_err(|e| tool_err(format!("cannot write to the server: {e}")))
    }

    /// Waits until the stderr reader reaches the end of the pipe or `bound` passes.
    fn drain_stderr(&self, bound: Duration) {
        let Ok(closed) = self.stderr_closed.lock() else {
            return;
        };
        let _ = self
            .stderr_closed_signal
            .wait_timeout_while(closed, bound, |closed| !*closed);
    }

    /// The stderr reader is done: wake anyone draining.
    fn close_stderr(&self) {
        if let Ok(mut closed) = self.stderr_closed.lock() {
            *closed = true;
        }
        self.stderr_closed_signal.notify_all();
    }

    /// The server's stderr for the error of a request that failed with the server gone: after the
    /// stream ended, the complete tail (waiting at most `STDERR_DRAIN` for it); otherwise what has
    /// arrived so far.
    fn stderr_tail_after_failure(&self) -> String {
        if self.stdout_closed.load(Ordering::SeqCst) {
            self.drain_stderr(STDERR_DRAIN);
        }
        self.stderr_tail()
    }

    /// The server's recent stderr as one string for an error message, or nothing.
    fn stderr_tail(&self) -> String {
        let lines = self.stderr.lock().map(|v| v.clone()).unwrap_or_default();
        if lines.is_empty() {
            String::new()
        } else {
            format!("; server stderr: {}", lines.join(" | "))
        }
    }

    fn fail_all(&self, why: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            for (_, tx) in pending.drain() {
                let _ = tx.send(Err(why.to_string()));
            }
        }
    }

    fn deliver(&self, message: Value) {
        // A message with a method is the server's: a notification is nothing to us; a request
        // gets an answer (`ping` a pong, anything else method-not-found) and never touches the
        // calls in flight.
        if let Some(method) = message.get("method").and_then(Value::as_str) {
            if let Some(id) = message.get("id") {
                let reply = if method == "ping" {
                    json!({ "jsonrpc": "2.0", "id": id, "result": {} })
                } else {
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32601, "message": "Method not found" } })
                };
                let _ = self.write(&reply);
            }
            return;
        }
        let Some(id) = message.get("id").and_then(Value::as_u64) else {
            // A response with no usable id: nobody asked for it.
            self.tainted.store(true, Ordering::SeqCst);
            self.fail_all("response without a numeric id");
            return;
        };
        if self
            .tombstones
            .lock()
            .map(|mut t| t.remove(&id))
            .unwrap_or(false)
        {
            return; // a late answer to a cancelled call
        }
        let waiting = self.pending.lock().ok().and_then(|mut p| p.remove(&id));
        match waiting {
            Some(tx) => {
                let _ = tx.send(Ok(message));
            }
            None => {
                self.tainted.store(true, Ordering::SeqCst);
                self.fail_all(&format!("response for unknown id {id}"));
            }
        }
    }
}

/// One server process, initialised and listed.
pub struct Client {
    child: Mutex<Child>,
    pid: u32,
    shared: Arc<Shared>,
    next_id: AtomicU64,
    tools: Vec<ListedTool>,
    server_info: Value,
}

fn tool_err(message: impl Into<String>) -> Error {
    Error::Tool(format!("mcp: {}", message.into()))
}

fn resolve_err(message: impl Into<String>) -> Error {
    Error::Config(format!("mcp: {}", message.into()))
}

impl Client {
    /// Spawns the server with the allowlisted environment, initialises it, and lists its tools,
    /// all before `deadline`. A wrong protocol version or a missing `tools` capability is
    /// `error: resolve`.
    pub fn connect<I, K, V>(spawn: &Spawn, environment: I, deadline: Instant) -> Result<Client>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut env = squint_core::env::allowlist(&spawn.declared_env, environment);
        env.extend(spawn.extra_env.iter().cloned());
        let mut command = Command::new(&spawn.command);
        command
            .args(&spawn.args)
            .env_clear()
            .envs(&env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &spawn.cwd {
            command.current_dir(cwd);
        }
        let mut child = command
            .spawn()
            .map_err(|e| resolve_err(format!("cannot start {}: {e}", spawn.command.display())))?;
        let stdin = child.stdin.take().ok_or_else(|| tool_err("no stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| tool_err("no stdout"))?;
        let stderr_pipe = child.stderr.take().ok_or_else(|| tool_err("no stderr"))?;
        let pid = child.id();

        let shared = Arc::new(Shared {
            pending: Mutex::new(HashMap::new()),
            tombstones: Mutex::new(BTreeSet::new()),
            tainted: AtomicBool::new(false),
            stderr: Mutex::new(Vec::new()),
            stderr_closed: Mutex::new(false),
            stderr_closed_signal: Condvar::new(),
            stdout_closed: AtomicBool::new(false),
            stdin: Mutex::new(Some(stdin)),
        });
        let reader_shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) if l.trim().is_empty() => continue,
                    Ok(l) => match serde_json::from_str::<Value>(&l) {
                        Ok(v) => reader_shared.deliver(v),
                        Err(_) => {
                            // A malformed frame ends the conversation.
                            reader_shared.tainted.store(true, Ordering::SeqCst);
                            reader_shared.fail_all("the server sent a malformed frame");
                            return;
                        }
                    },
                    Err(_) => break,
                }
            }
            reader_shared.tainted.store(true, Ordering::SeqCst);
            reader_shared.stdout_closed.store(true, Ordering::SeqCst);
            reader_shared.fail_all("the server ended the conversation");
        });
        let sink = Arc::clone(&shared);
        std::thread::spawn(move || {
            read_stderr_lines(BufReader::new(stderr_pipe), |line| {
                if let Ok(mut v) = sink.stderr.lock() {
                    if v.len() >= STDERR_LINES {
                        v.remove(0);
                    }
                    v.push(line);
                }
            });
            sink.close_stderr();
        });

        let mut client = Client {
            child: Mutex::new(child),
            pid,
            shared,
            next_id: AtomicU64::new(1),
            tools: Vec::new(),
            server_info: Value::Null,
        };
        if let Err(e) = client.initialize(deadline) {
            client.shutdown(Duration::from_millis(200));
            return Err(e);
        }
        Ok(client)
    }

    fn initialize(&mut self, deadline: Instant) -> Result<()> {
        let result = self.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "squint", "version": env!("CARGO_PKG_VERSION") },
            }),
            deadline,
        )?;
        let version = result["protocolVersion"].as_str().unwrap_or("");
        if version != PROTOCOL_VERSION {
            return Err(resolve_err(format!(
                "the server speaks protocol {version:?}; this release requires {PROTOCOL_VERSION:?}"
            )));
        }
        if !result["capabilities"]["tools"].is_object() {
            return Err(resolve_err(
                "the server does not advertise the `tools` capability",
            ));
        }
        self.server_info = result["serverInfo"].clone();
        self.notify("notifications/initialized", json!({}))?;
        self.tools = self.list_tools(deadline)?;
        Ok(())
    }

    /// `tools/list` following `nextCursor` until the server stops giving one; a cursor that
    /// repeats, or more than `MAX_LIST_PAGES` pages, is refused.
    fn list_tools(&self, deadline: Instant) -> Result<Vec<ListedTool>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for _ in 0..MAX_LIST_PAGES {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let page = self.request("tools/list", params, deadline)?;
            for t in page["tools"].as_array().cloned().unwrap_or_default() {
                tools.push(listed_tool(&t)?);
            }
            match page["nextCursor"].as_str() {
                Some(c) if !c.is_empty() => {
                    if !seen.insert(c.to_string()) {
                        return Err(resolve_err(format!(
                            "tools/list repeats the cursor {c:?}; the listing never ends"
                        )));
                    }
                    cursor = Some(c.to_string());
                }
                _ => return Ok(tools),
            }
        }
        Err(resolve_err(format!(
            "tools/list ran past {MAX_LIST_PAGES} pages; the listing never ends"
        )))
    }

    pub fn tools(&self) -> &[ListedTool] {
        &self.tools
    }

    pub fn manifests(&self) -> Vec<Manifest> {
        self.tools.iter().map(|t| t.manifest.clone()).collect()
    }

    /// The manifest of a listed tool.
    pub fn manifest(&self, name: &str) -> Option<&Manifest> {
        self.tools
            .iter()
            .find(|t| t.name == name)
            .map(|t| &t.manifest)
    }

    /// Whether a call in this process timed out or was cancelled, or its stream broke; such a
    /// process is never reused.
    pub fn is_tainted(&self) -> bool {
        self.shared.tainted.load(Ordering::SeqCst)
    }

    /// The server's stderr so far (for the journal).
    pub fn stderr_lines(&self) -> Vec<String> {
        self.shared
            .stderr
            .lock()
            .map(|v| v.clone())
            .unwrap_or_default()
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    fn send(&self, message: &Value) -> Result<()> {
        self.shared.write(message)
    }

    fn notify(&self, method: &str, params: Value) -> Result<()> {
        self.send(&json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    /// Sends a request and waits for its response until `deadline`. A timeout cancels the request
    /// and taints the process; a broken stream or a stray response fails it.
    fn request(&self, method: &str, params: Value, deadline: Instant) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx): (Sender<Delivery>, Receiver<Delivery>) = channel();
        self.shared
            .pending
            .lock()
            .map_err(|_| tool_err("pending requests poisoned"))?
            .insert(id, tx);
        if let Err(e) =
            self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))
        {
            if let Ok(mut p) = self.shared.pending.lock() {
                p.remove(&id);
            }
            // A server that cannot be written to has usually exited, perhaps before the stdout
            // reader saw the end of the stream; its stderr says why.
            return Err(match e {
                Error::Tool(m) => {
                    self.shared.drain_stderr(STDERR_DRAIN);
                    Error::Tool(format!("{m}{}", self.shared.stderr_tail()))
                }
                other => other,
            });
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining) {
            Ok(Ok(m)) => {
                if let Some(err) = m.get("error") {
                    return Err(Error::Tool(format!(
                        "mcp: {method} failed: {} (code {})",
                        err["message"].as_str().unwrap_or("?"),
                        err["code"]
                    )));
                }
                Ok(m.get("result").cloned().unwrap_or(Value::Null))
            }
            Ok(Err(why)) => Err(tool_err(format!(
                "{method} (id {id}): {why}{}",
                self.shared.stderr_tail_after_failure()
            ))),
            Err(RecvTimeoutError::Timeout) => Err(self.cancel(id, method)),
            Err(RecvTimeoutError::Disconnected) => {
                self.shared.tainted.store(true, Ordering::SeqCst);
                Err(tool_err(format!(
                    "the server died during {method}{}",
                    self.shared.stderr_tail_after_failure()
                )))
            }
        }
    }

    fn cancel(&self, id: u64, method: &str) -> Error {
        // The tombstone goes down before the pending entry, so a late answer landing between
        // the two is a late answer, never a stray id that fails the other calls in flight.
        if let Ok(mut t) = self.shared.tombstones.lock() {
            t.insert(id);
        }
        if let Ok(mut p) = self.shared.pending.lock() {
            p.remove(&id);
        }
        self.shared.tainted.store(true, Ordering::SeqCst);
        let _ = self.notify(
            "notifications/cancelled",
            json!({ "requestId": id, "reason": "deadline" }),
        );
        tool_err(format!("{method} (id {id}) timed out; cancelled"))
    }

    /// `tools/call` with the full arguments (model arguments plus `_squint`). Several calls may
    /// be in flight at once; each keeps its own deadline.
    pub fn call(&self, name: &str, arguments: Value, timeout: Duration) -> Result<CallResult> {
        let deadline = Instant::now() + timeout;
        let result = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
            deadline,
        )?;
        Ok(CallResult {
            structured: result
                .get("structuredContent")
                .cloned()
                .unwrap_or(Value::Null),
            content: result["content"].as_array().cloned().unwrap_or_default(),
            is_error: result["isError"].as_bool().unwrap_or(false),
        })
    }

    /// `squint/reset`: the server drops every prior-run task and mutable state before answering.
    pub fn reset(&self, deadline: Instant) -> Result<Reset> {
        match self.request("squint/reset", json!({}), deadline) {
            Ok(_) => Ok(Reset::Acknowledged),
            Err(Error::Tool(m)) if m.contains("(code -32601)") => Ok(Reset::Unsupported),
            Err(e) => Err(e),
        }
    }

    /// Close stdin, wait, SIGTERM, wait, SIGKILL.
    pub fn shutdown(&self, grace: Duration) {
        if let Ok(mut s) = self.shared.stdin.lock() {
            drop(s.take());
        }
        let Ok(mut child) = self.child.lock() else {
            return;
        };
        if wait_for_exit(&mut child, grace) {
            return;
        }
        #[cfg(unix)]
        {
            let _ = Command::new("kill")
                .args(["-TERM", &self.pid.to_string()])
                .status();
            if wait_for_exit(&mut child, grace) {
                return;
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn wait_for_exit(child: &mut Child, grace: Duration) -> bool {
    let deadline = Instant::now() + grace;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return true,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => return false,
        }
    }
}

/// A listed MCP tool to a squint manifest: `name`, `inputSchema` and `outputSchema` from the
/// native fields, the rest from the `squint` annotation; `description` must equal `purpose`.
fn listed_tool(tool: &Value) -> Result<ListedTool> {
    let name = tool["name"]
        .as_str()
        .ok_or_else(|| resolve_err("a listed tool has no name"))?
        .to_string();
    let mut manifest = tool["annotations"]["squint"]
        .as_object()
        .cloned()
        .ok_or_else(|| {
            resolve_err(format!(
                "tool {name:?} carries no `squint` manifest in its annotations"
            ))
        })?;
    manifest.insert("name".into(), Value::String(name.clone()));
    if let Some(schema) = tool.get("inputSchema") {
        manifest.insert("input_schema".into(), schema.clone());
    }
    if let Some(schema) = tool.get("outputSchema") {
        manifest.insert("output_schema".into(), schema.clone());
    }
    let manifest = Manifest::from_json(&Value::Object(manifest))?;
    let description = tool["description"].as_str().unwrap_or("");
    if description != manifest.purpose {
        return Err(resolve_err(format!(
            "tool {name:?}: description must equal the manifest's purpose"
        )));
    }
    Ok(ListedTool { name, manifest })
}

/// A call result to an envelope (SPEC "Results"): `structuredContent` is the envelope, validated
/// against the tool's `outputSchema`; the content blocks are exactly the image blocks of
/// `images`, in order and media type, and nothing else. `isError` with an `invalid_args` envelope
/// is the one recoverable form; `isError` with any other status, a block mismatch or a schema
/// breach is `error: tool`.
pub fn envelope_from(result: CallResult, manifest: &Manifest) -> Result<Envelope> {
    let tool = &manifest.name;
    let mut envelope: Envelope =
        serde_json::from_value(result.structured.clone()).map_err(|e| {
            tool_err(format!(
                "{tool}: structuredContent is not a result envelope: {e}"
            ))
        })?;
    if result.is_error && envelope.status != Status::InvalidArgs {
        return Err(tool_err(format!(
            "{tool}: isError with status {:?}: {}",
            envelope.status, envelope.summary
        )));
    }
    let validator = jsonschema::validator_for(&manifest.output_schema)
        .map_err(|e| tool_err(format!("{tool}: outputSchema is not a valid schema: {e}")))?;
    let breaches: Vec<String> = validator
        .iter_errors(&result.structured)
        .map(|e| format!("{} at {}", e, e.instance_path()))
        .collect();
    if !breaches.is_empty() {
        return Err(tool_err(format!(
            "{tool}: the result does not match its outputSchema: {}",
            breaches.join("; ")
        )));
    }
    if let Some(stray) = result.content.iter().find(|b| b["type"] != "image") {
        return Err(tool_err(format!(
            "{tool}: content holds a {:?} block; only the image blocks of `images` belong there",
            stray["type"].as_str().unwrap_or("untyped")
        )));
    }
    if result.content.len() != envelope.images.len() {
        return Err(tool_err(format!(
            "{tool}: {} image blocks for {} envelope images",
            result.content.len(),
            envelope.images.len()
        )));
    }
    for (block, image) in result.content.iter().zip(envelope.images.iter_mut()) {
        let mime = block["mimeType"].as_str().unwrap_or("");
        if mime != image.media_type {
            return Err(tool_err(format!(
                "{tool}: image block {mime:?} for envelope image {:?}",
                image.media_type
            )));
        }
        image.base64 = block["data"].as_str().map(str::to_string);
    }
    Ok(envelope)
}

/// Environment pairs the harness passes to a spawn: the process environment by default.
pub fn process_environment() -> BTreeMap<String, String> {
    std::env::vars().collect()
}

#[cfg(test)]
mod full;
#[cfg(test)]
mod minimal;
